//! Connects the capture to the meeting recorder in `debrief-core`.

use std::path::{Path, PathBuf};

use debrief_core::recorder::{CaptureControl, Sink, Sources};
use debrief_core::recording::Track;

use crate::{AudioChunk, Capture, CaptureError, Handler, Source};

impl CaptureControl for Capture {
    type Error = CaptureError;

    fn now(&self) -> f64 {
        Capture::now(self)
    }

    fn sources(&self) -> Sources {
        let info = self.info();
        Sources {
            microphone: info.microphone.clone(),
            system_audio: info.system_audio.clone(),
            video: info.video.clone(),
        }
    }

    fn video_bytes(&self) -> Option<u64> {
        Capture::video_bytes(self)
    }

    fn pause_video(&mut self) -> Result<(), CaptureError> {
        Capture::pause_video(self)
    }

    fn resume_video(&mut self, path: &Path) -> Result<(), CaptureError> {
        Capture::resume_video(self, path)
    }

    fn stop(self) -> Result<(), CaptureError> {
        Capture::stop(self)
    }

    fn join_videos(parts: &[(PathBuf, f64)], output: &Path) -> Result<(), CaptureError> {
        crate::join_videos(parts, output)
    }
}

impl Handler for Sink {
    fn audio(&self, chunk: AudioChunk<'_>) {
        let track = match chunk.source {
            Source::Microphone => Track::Me,
            Source::System => Track::Them,
        };
        self.write(track, chunk.time, chunk.sample_rate, chunk.samples);
    }

    fn failed(&self, message: String) {
        self.fail(message);
    }
}
