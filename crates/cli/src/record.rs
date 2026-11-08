use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, bail};
use chrono::Local;
use debrief_capture::{
    AudioChunk, Capture, Handler, Options, Source, SystemAudio, Video, VideoTarget,
};
use debrief_core::meeting::{
    self, Language, MEETING_FILE, Meeting, Settings, VIDEO_FILE, VideoFile,
};
use debrief_core::recording::{Track, TrackStats};
use debrief_core::session::Session;

use crate::format::{clock, parse_duration, size};

const STATUS_INTERVAL: Duration = Duration::from_millis(500);

#[derive(clap::Args)]
pub struct Args {
    /// Meeting title, also used in the folder name.
    #[arg(long, default_value = "")]
    title: String,
    /// Language of the meeting for transcription: auto, ru or en.
    #[arg(long, default_value = "auto")]
    language: Language,
    /// Folder for the recording. Default: a new folder in ~/Documents/Debrief named
    /// after the date, time and title.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Microphone ID from `debrief sources`. Default: the system default.
    #[arg(long)]
    mic: Option<String>,
    /// Record system audio from one app only, by bundle ID (for example us.zoom.xos).
    #[arg(long)]
    app: Option<String>,
    /// Record this screen, by ID. Default: the main screen.
    #[arg(long, conflicts_with_all = ["window", "no_video"])]
    screen: Option<u32>,
    /// Record this window, by ID, instead of a screen.
    #[arg(long, conflicts_with = "no_video")]
    window: Option<u32>,
    /// Record audio only.
    #[arg(long)]
    no_video: bool,
    /// Video frames per second.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=30))]
    fps: u32,
    /// Stop after this much recorded time, for example 90s, 30m or 1h. Default: when
    /// you press Ctrl+C.
    #[arg(long, value_parser = parse_duration)]
    duration: Option<Duration>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let pressed_at = Local::now().fixed_offset();
    let folder = match &args.out {
        Some(folder) => {
            fs::create_dir_all(folder)
                .with_context(|| format!("could not create {}", folder.display()))?;
            if folder.join(MEETING_FILE).exists() || folder.join(Track::Me.file_name()).exists() {
                bail!("{} already has a recording", folder.display());
            }
            folder.clone()
        }
        None => meeting::create_folder(
            &library_folder()?,
            &meeting::folder_name(&pressed_at, &args.title),
        )?,
    };

    let mut draft = Meeting::new(
        args.title.clone(),
        pressed_at,
        Settings {
            language: args.language,
        },
    );
    draft.video = (!args.no_video).then(|| VideoFile {
        file: VIDEO_FILE.to_string(),
        source: String::new(),
        fps: args.fps,
        bytes: None,
        parts: Vec::new(),
    });
    let session = match Session::create(&folder, draft) {
        Ok(session) => session,
        Err(error) => {
            let _ = fs::remove_dir(&folder);
            return Err(error.into());
        }
    };

    let target = match (args.screen, args.window) {
        (Some(id), _) => VideoTarget::Display(id),
        (_, Some(id)) => VideoTarget::Window(id),
        _ => VideoTarget::MainDisplay,
    };
    let options = Options {
        microphone: args.mic.clone(),
        system_audio: match &args.app {
            Some(bundle_id) => SystemAudio::App {
                bundle_id: bundle_id.clone(),
            },
            None => SystemAudio::AllApps,
        },
        video: session.video_part().map(|path| Video {
            target,
            fps: args.fps,
            path,
        }),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let on_ctrl_c = Arc::clone(&stop);
    ctrlc::set_handler(move || on_ctrl_c.store(true, Ordering::SeqCst))
        .context("could not catch Ctrl+C")?;

    let recorder = Arc::new(Recorder {
        session: Mutex::new(Some(session)),
        error: Mutex::new(None),
    });
    let mut capture = match Capture::start(&options, recorder.clone()) {
        Ok(capture) => capture,
        Err(error) => {
            if let Some(session) = lock(&recorder.session).take() {
                session.discard();
            }
            let _ = fs::remove_dir(&folder);
            return Err(error.into());
        }
    };
    let info = capture.info().clone();

    let started_at = Local::now().fixed_offset();
    let saved = recorder.with_session(|session| {
        session.update(|meeting| {
            meeting.recording.started_at = started_at;
            meeting.tracks.me.source = info.microphone.clone();
            meeting.tracks.them.source = info.system_audio.clone();
            if let (Some(video), Some(source)) = (&mut meeting.video, &info.video) {
                video.source = source.clone();
            }
        })
    });
    if let Some(Err(error)) = saved {
        let _ = capture.stop();
        return Err(error.into());
    }

    println!("Recording to {}", folder.display());
    println!("  Me:    {}", info.microphone);
    println!("  Them:  {}", info.system_audio);
    println!("  Video: {}", info.video.as_deref().unwrap_or("off"));
    match args.duration {
        Some(duration) => println!(
            "Stops after {} or on Ctrl+C.",
            clock(duration.as_secs_f64())
        ),
        None => println!("Press Ctrl+C to stop."),
    }
    println!("Type p and Enter to pause or resume, m and Enter to mark a moment.");

    let commands = read_commands();
    loop {
        match commands.recv_timeout(STATUS_INTERVAL) {
            Ok(Command::Pause) => {
                if let Err(error) = toggle_pause(&recorder, &mut capture) {
                    println!("\n{error:#}");
                }
            }
            Ok(Command::Mark) => {
                let at = capture.now();
                match recorder.with_session(|session| session.mark(at)) {
                    Some(Ok(mark)) => println!("\nMarked {}", clock(mark.at_seconds)),
                    Some(Err(error)) => println!("\nCould not save the mark: {error}"),
                    None => {}
                }
            }
            Ok(Command::Stop) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => thread::sleep(STATUS_INTERVAL),
        }
        let recorded = print_status(&recorder, &capture);
        let time_is_up = args
            .duration
            .is_some_and(|duration| recorded >= duration.as_secs_f64());
        if stop.load(Ordering::SeqCst) || time_is_up || lock(&recorder.error).is_some() {
            break;
        }
    }
    println!();

    let stop_at = capture.now();
    let stopped = capture.stop();
    let error = lock(&recorder.error).clone();
    let session = lock(&recorder.session)
        .take()
        .context("the recording was already closed")?;
    let mut meeting = session.finish(stop_at, error.clone())?;
    let video = meeting::finish_video(&folder, &mut meeting, debrief_capture::join_videos);
    meeting.save(&folder)?;
    print_summary(&folder, &meeting);
    if let Err(error) = video {
        println!("{error}. The parts are kept and listed in {MEETING_FILE}.");
    }

    stopped?;
    if let Some(error) = error {
        bail!("recording stopped early: {error}");
    }
    Ok(())
}

enum Command {
    Pause,
    Mark,
    Stop,
}

fn read_commands() -> Receiver<Command> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lines() {
            let command = match line.as_deref().map(str::trim) {
                Ok("p") => Command::Pause,
                Ok("m") => Command::Mark,
                Ok("q") => Command::Stop,
                Ok(_) => continue,
                Err(_) => return,
            };
            if sender.send(command).is_err() {
                return;
            }
        }
    });
    receiver
}

fn toggle_pause(recorder: &Recorder, capture: &mut Capture) -> anyhow::Result<()> {
    let at = capture.now();
    let mut guard = lock(&recorder.session);
    let Some(session) = guard.as_mut() else {
        return Ok(());
    };
    if session.is_paused() {
        let next = session.resume(at)?;
        drop(guard);
        if let Some(path) = next {
            capture
                .resume_video(&path)
                .context("the audio resumed, but the video could not")?;
        }
        println!("\nResumed.");
    } else {
        session.pause(at);
        let position = session.position(at);
        drop(guard);
        capture
            .pause_video()
            .context("the audio is paused, but the video could not be")?;
        println!(
            "\nPaused at {}. Type p and Enter to resume.",
            clock(position)
        );
    }
    Ok(())
}

struct Recorder {
    session: Mutex<Option<Session>>,
    error: Mutex<Option<String>>,
}

impl Recorder {
    fn with_session<T>(&self, action: impl FnOnce(&mut Session) -> T) -> Option<T> {
        lock(&self.session).as_mut().map(action)
    }
}

impl Handler for Recorder {
    fn audio(&self, chunk: AudioChunk<'_>) {
        let track = match chunk.source {
            Source::Microphone => Track::Me,
            Source::System => Track::Them,
        };
        let written = self.with_session(|session| {
            session.audio(track, chunk.time, chunk.sample_rate, chunk.samples)
        });
        if let Some(Err(error)) = written {
            self.failed(error.to_string());
        }
    }

    fn failed(&self, message: String) {
        lock(&self.error).get_or_insert(message);
    }
}

/// Returns the recorded time.
fn print_status(recorder: &Recorder, capture: &Capture) -> f64 {
    let at = capture.now();
    let Some((recorded, paused, marks, me, them)) = recorder.with_session(|session| {
        (
            session.position(at),
            session.is_paused(),
            session.meeting().marks.len(),
            session.stats(Track::Me).clone(),
            session.stats(Track::Them).clone(),
        )
    }) else {
        return 0.0;
    };
    let video = capture
        .video_bytes()
        .map(|bytes| format!("  video {}", size(bytes)))
        .unwrap_or_default();
    print!(
        "\r{}{}  me {}  them {}{video}  marks {marks}  timing fixes {}/{}   ",
        if paused { "Paused " } else { "" },
        clock(recorded),
        size(me.bytes()),
        size(them.bytes()),
        fixes(&me),
        fixes(&them),
    );
    let _ = io::stdout().flush();
    recorded
}

fn print_summary(folder: &Path, meeting: &Meeting) {
    println!("Saved to {}", folder.display());
    for track in [&meeting.tracks.me, &meeting.tracks.them] {
        let Some(stats) = &track.stats else {
            continue;
        };
        let first = stats
            .first_chunk_at
            .map(|at| format!("first audio at {at:.3} s"))
            .unwrap_or_else(|| "no audio".into());
        println!(
            "  {:<12} {}  {}  {first}, {} timing fixes, rates {:?}",
            track.file,
            clock(stats.duration_seconds()),
            size(stats.bytes()),
            fixes(stats),
            stats.source_rates,
        );
    }
    if let Some(video) = &meeting.video {
        if video.parts.is_empty() {
            let bytes = video.bytes.map(size).unwrap_or_else(|| "missing".into());
            println!("  {:<12} {bytes}", video.file);
        } else {
            for part in &video.parts {
                println!("  {:<12} from {}", part.file, clock(part.at_seconds));
            }
        }
    }
    println!("  {MEETING_FILE}");
    let mut recorded = format!(
        "Recorded {}, {}",
        clock(meeting.recording.duration_seconds),
        count(meeting.marks.len(), "mark", "marks"),
    );
    if !meeting.pauses.is_empty() {
        let paused: f64 = meeting
            .pauses
            .iter()
            .map(|pause| pause.duration_seconds)
            .sum();
        recorded.push_str(&format!(
            ", {} ({} in all)",
            count(meeting.pauses.len(), "pause", "pauses"),
            clock(paused),
        ));
    }
    println!("{recorded}");
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn fixes(stats: &TrackStats) -> u64 {
    stats.gaps_filled + stats.overlaps_trimmed
}

fn library_folder() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set; pass --out")?;
    Ok(PathBuf::from(home).join("Documents").join("Debrief"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
