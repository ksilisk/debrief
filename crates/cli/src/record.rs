use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use chrono::Local;
use debrief_capture::{
    AudioChunk, Capture, Handler, Options, Source, SystemAudio, Video, VideoTarget,
};
use debrief_core::recording::{RecordedTracks, Track, TrackStats, Tracks};
use serde::Serialize;

use crate::format::{clock, parse_duration, size};

const VIDEO_FILE: &str = "screen.mp4";
const LOG_FILE: &str = "record-log.json";

#[derive(clap::Args)]
pub struct Args {
    /// Folder for the recording. Default: ~/Documents/Debrief/<date and time>.
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
    /// Stop after this long, for example 90s, 30m or 1h. Default: when you press Ctrl+C.
    #[arg(long, value_parser = parse_duration)]
    duration: Option<Duration>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let folder = match &args.out {
        Some(folder) => folder.clone(),
        None => default_folder()?,
    };
    fs::create_dir_all(&folder)
        .with_context(|| format!("could not create {}", folder.display()))?;
    if folder.join(Track::Me.file_name()).exists() {
        bail!("{} already has a recording", folder.display());
    }

    let video_path = folder.join(VIDEO_FILE);
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
        video: (!args.no_video).then(|| Video {
            target,
            fps: args.fps,
            path: video_path.clone(),
        }),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let on_ctrl_c = Arc::clone(&stop);
    ctrlc::set_handler(move || on_ctrl_c.store(true, Ordering::SeqCst))
        .context("could not catch Ctrl+C")?;

    let recorder = Arc::new(Recorder {
        tracks: Mutex::new(Some(Tracks::create(&folder)?)),
        error: Mutex::new(None),
    });
    let started_at = Local::now();
    let capture = match Capture::start(&options, recorder.clone()) {
        Ok(capture) => capture,
        Err(error) => {
            drop(lock(&recorder.tracks).take());
            for track in [Track::Me, Track::Them] {
                let _ = fs::remove_file(folder.join(track.file_name()));
            }
            let _ = fs::remove_dir(&folder);
            return Err(error.into());
        }
    };
    let info = capture.info().clone();

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

    let start = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let elapsed = start.elapsed();
        print_status(elapsed, &recorder, capture.video_bytes());
        let time_is_up = args.duration.is_some_and(|duration| elapsed >= duration);
        if stop.load(Ordering::SeqCst) || time_is_up || lock(&recorder.error).is_some() {
            break;
        }
    }
    println!();

    let duration_seconds = start.elapsed().as_secs_f64();
    let stopped = capture.stop();
    let tracks = lock(&recorder.tracks)
        .take()
        .context("the tracks were already closed")?
        .finish()?;
    let error = lock(&recorder.error).clone();
    let video_bytes = fs::metadata(&video_path).map(|meta| meta.len()).ok();

    let log = RecordLog {
        debrief_version: debrief_core::VERSION,
        started_at: started_at.to_rfc3339(),
        duration_seconds,
        microphone: &info.microphone,
        system_audio: &info.system_audio,
        video: info.video.as_deref().map(|description| VideoLog {
            description,
            file: VIDEO_FILE,
            bytes: video_bytes,
        }),
        tracks: &tracks,
        error: error.as_deref(),
    };
    write_log(&folder.join(LOG_FILE), &log)?;
    print_summary(&folder, &tracks, video_bytes);

    stopped?;
    if let Some(error) = error {
        bail!("recording stopped early: {error}");
    }
    Ok(())
}

struct Recorder {
    tracks: Mutex<Option<Tracks>>,
    error: Mutex<Option<String>>,
}

impl Handler for Recorder {
    fn audio(&self, chunk: AudioChunk<'_>) {
        let track = match chunk.source {
            Source::Microphone => Track::Me,
            Source::System => Track::Them,
        };
        let mut tracks = lock(&self.tracks);
        if let Some(tracks) = tracks.as_mut() {
            if let Err(error) = tracks.write(track, chunk.time, chunk.sample_rate, chunk.samples) {
                self.failed(error.to_string());
            }
        }
    }

    fn failed(&self, message: String) {
        lock(&self.error).get_or_insert(message);
    }
}

#[derive(Serialize)]
struct RecordLog<'a> {
    debrief_version: &'a str,
    started_at: String,
    duration_seconds: f64,
    microphone: &'a str,
    system_audio: &'a str,
    video: Option<VideoLog<'a>>,
    tracks: &'a RecordedTracks,
    error: Option<&'a str>,
}

#[derive(Serialize)]
struct VideoLog<'a> {
    description: &'a str,
    file: &'a str,
    bytes: Option<u64>,
}

fn print_status(elapsed: Duration, recorder: &Recorder, video_bytes: Option<u64>) {
    let (me, them) = match lock(&recorder.tracks).as_ref() {
        Some(tracks) => (
            tracks.stats(Track::Me).clone(),
            tracks.stats(Track::Them).clone(),
        ),
        None => return,
    };
    let video = video_bytes
        .map(|bytes| format!("  video {}", size(bytes)))
        .unwrap_or_default();
    print!(
        "\r{}  me {}  them {}{video}  timing fixes {}/{}   ",
        clock(elapsed.as_secs_f64()),
        size(me.bytes()),
        size(them.bytes()),
        fixes(&me),
        fixes(&them),
    );
    let _ = std::io::stdout().flush();
}

fn print_summary(folder: &Path, tracks: &RecordedTracks, video_bytes: Option<u64>) {
    println!("Saved to {}", folder.display());
    for (track, stats) in [(Track::Me, &tracks.me), (Track::Them, &tracks.them)] {
        let first = stats
            .first_chunk_at
            .map(|at| format!("first audio at {at:.3} s"))
            .unwrap_or_else(|| "no audio".into());
        println!(
            "  {:<10} {}  {}  {first}, {} timing fixes, rates {:?}",
            track.file_name(),
            clock(stats.duration_seconds()),
            size(stats.bytes()),
            fixes(stats),
            stats.source_rates,
        );
    }
    if let Some(bytes) = video_bytes {
        println!("  {VIDEO_FILE:<10} {}", size(bytes));
    }
    println!("  {LOG_FILE}");
}

fn fixes(stats: &TrackStats) -> u64 {
    stats.gaps_filled + stats.overlaps_trimmed
}

fn write_log(path: &Path, log: &RecordLog<'_>) -> anyhow::Result<()> {
    let file = File::create(path).with_context(|| format!("could not write {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, log)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn default_folder() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set; pass --out")?;
    let name = Local::now().format("%Y-%m-%d %H-%M-%S").to_string();
    Ok(PathBuf::from(home)
        .join("Documents")
        .join("Debrief")
        .join(name))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
