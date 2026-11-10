use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use anyhow::{Context, bail};
use chrono::Local;
use debrief_capture::{Capture, Options, SystemAudio, Video, VideoTarget};
use debrief_core::meeting::{
    self, Language, MEETING_FILE, Meeting, Settings, VIDEO_FILE, VideoFile,
};
use debrief_core::recorder::{Recorder, Status};
use debrief_core::recording::{Track, TrackStats};

use crate::format::{clock, parse_duration, size};

const STATUS_INTERVAL: Duration = Duration::from_millis(500);

#[derive(clap::Args)]
pub struct Args {
    /// Meeting title, also used in the folder name.
    #[arg(long, default_value = "")]
    title: String,
    /// Language of the meeting for transcription: ru, en, or auto (one language per
    /// track, detected from its first 30 seconds).
    #[arg(long, default_value = "ru")]
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
    let target = match (args.screen, args.window) {
        (Some(id), _) => VideoTarget::Display(id),
        (_, Some(id)) => VideoTarget::Window(id),
        _ => VideoTarget::MainDisplay,
    };

    let stop = Arc::new(AtomicBool::new(false));
    let on_ctrl_c = Arc::clone(&stop);
    ctrlc::set_handler(move || on_ctrl_c.store(true, Ordering::SeqCst))
        .context("could not catch Ctrl+C")?;

    let mut recorder = Recorder::start(&folder, draft, |video, sink| {
        let options = Options {
            microphone: args.mic.clone(),
            system_audio: match &args.app {
                Some(bundle_id) => SystemAudio::App {
                    bundle_id: bundle_id.clone(),
                },
                None => SystemAudio::AllApps,
            },
            video: video.map(|path| Video {
                target,
                fps: args.fps,
                path,
            }),
        };
        Capture::start(&options, sink)
    })?;
    let sources = recorder.sources().clone();

    println!("Recording to {}", folder.display());
    println!("  Me:    {}", sources.microphone);
    println!("  Them:  {}", sources.system_audio);
    println!("  Video: {}", sources.video.as_deref().unwrap_or("off"));
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
            Ok(Command::Pause) => match recorder.toggle_pause() {
                Ok(true) => println!(
                    "\nPaused at {}. Type p and Enter to resume.",
                    clock(recorder.status().position)
                ),
                Ok(false) => println!("\nResumed."),
                Err(error) => println!("\n{error}"),
            },
            Ok(Command::Mark) => match recorder.mark() {
                Ok(mark) => println!("\nMarked {}", clock(mark.at_seconds)),
                Err(error) => println!("\nCould not save the mark: {error}"),
            },
            Ok(Command::Stop) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => thread::sleep(STATUS_INTERVAL),
        }
        let status = recorder.status();
        print_status(&status);
        let time_is_up = args
            .duration
            .is_some_and(|duration| status.position >= duration.as_secs_f64());
        if stop.load(Ordering::SeqCst) || time_is_up || status.error.is_some() {
            break;
        }
    }
    println!();

    let stopped = recorder.stop()?;
    print_summary(&stopped.folder, &stopped.meeting);
    if let Some(error) = stopped.video_error {
        println!("{error}. The parts are kept and listed in {MEETING_FILE}.");
    }
    if let Some(error) = stopped.capture_error {
        return Err(anyhow::Error::from_boxed(error));
    }
    if let Some(error) = stopped.meeting.recording.error {
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

fn print_status(status: &Status) {
    let video = status
        .video_bytes
        .map(|bytes| format!("  video {}", size(bytes)))
        .unwrap_or_default();
    print!(
        "\r{}{}  me {}  them {}{video}  marks {}  timing fixes {}/{}   ",
        if status.paused { "Paused " } else { "" },
        clock(status.position),
        size(status.me.bytes()),
        size(status.them.bytes()),
        status.marks,
        fixes(&status.me),
        fixes(&status.them),
    );
    let _ = io::stdout().flush();
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
    debrief_summarize::config::library().context("could not find the meetings folder; pass --out")
}
