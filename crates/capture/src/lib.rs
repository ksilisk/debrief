//! Recording of the microphone, system audio and screen. Each OS gets its own
//! module behind this interface.

use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
use unsupported as platform;

mod recorder;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("recording is not supported on this system yet")]
    Unsupported,
    #[error(
        "no permission to record the screen and system audio: {0}. Allow the app you run \
         `debrief` from (for example Terminal) in System Settings > Privacy & Security > \
         Screen & System Audio Recording, then restart that app"
    )]
    Permission(String),
    #[error("{0} was not found. Run `debrief sources` to see what is available")]
    NotFound(String),
    #[error("{0}")]
    Platform(String),
}

#[derive(Debug, Clone)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
}

#[derive(Debug, Clone)]
pub struct Display {
    pub id: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Window {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Sources {
    pub microphones: Vec<Microphone>,
    pub apps: Vec<App>,
    pub displays: Vec<Display>,
    pub windows: Vec<Window>,
}

pub fn sources() -> Result<Sources, CaptureError> {
    platform::sources()
}

/// Joins video files into one without re-encoding. Each part is placed at its
/// start in seconds; a gap before a part stays empty.
pub fn join_videos(parts: &[(PathBuf, f64)], output: &Path) -> Result<(), CaptureError> {
    platform::join_videos(parts, output)
}

#[derive(Debug, Clone)]
pub enum SystemAudio {
    AllApps,
    /// Also includes the app's helper processes (bundle IDs that start with
    /// `bundle_id.`). The app may start after the recording does.
    App {
        bundle_id: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum VideoTarget {
    MainDisplay,
    Display(u32),
    Window(u32),
}

#[derive(Debug, Clone)]
pub struct Video {
    pub target: VideoTarget,
    pub fps: u32,
    /// The first file; after a pause, [`Capture::resume_video`] names the next one.
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// A microphone ID from [`sources`]; `None` uses the system default.
    pub microphone: Option<String>,
    pub system_audio: SystemAudio,
    pub video: Option<Video>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Microphone,
    System,
}

/// Mono audio. `time` is in seconds on a clock shared by both sources.
#[derive(Debug)]
pub struct AudioChunk<'a> {
    pub source: Source,
    pub time: f64,
    pub sample_rate: u32,
    pub samples: &'a [f32],
}

/// Called from capture threads.
pub trait Handler: Send + Sync + 'static {
    fn audio(&self, chunk: AudioChunk<'_>);
    fn failed(&self, message: String);
}

/// What the capture actually uses, after defaults and lookups.
#[derive(Debug, Clone)]
pub struct CaptureInfo {
    pub microphone: String,
    pub system_audio: String,
    pub video: Option<String>,
}

pub struct Capture(platform::Capture);

impl Capture {
    pub fn start(options: &Options, handler: Arc<dyn Handler>) -> Result<Self, CaptureError> {
        platform::Capture::start(options, handler).map(Self)
    }

    pub fn info(&self) -> &CaptureInfo {
        self.0.info()
    }

    /// Seconds on the clock that audio chunk times use.
    pub fn now(&self) -> f64 {
        self.0.now()
    }

    /// Total size of the video files so far.
    pub fn video_bytes(&self) -> Option<u64> {
        self.0.video_bytes()
    }

    /// Closes the current video file. The screen is not recorded until
    /// [`Capture::resume_video`].
    pub fn pause_video(&mut self) -> Result<(), CaptureError> {
        self.0.pause_video()
    }

    /// Continues the video in a new file.
    pub fn resume_video(&mut self, path: &Path) -> Result<(), CaptureError> {
        self.0.resume_video(path)
    }

    /// Stops recording and waits until the video file is complete.
    pub fn stop(self) -> Result<(), CaptureError> {
        self.0.stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_can_be_shared_between_threads() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Capture>();
    }
}
