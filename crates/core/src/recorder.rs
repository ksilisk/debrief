//! One recording from start to stop: the session, the capture, pauses and marks.
//! The capture is behind [`CaptureControl`], so this module has no platform code
//! and the tests use a fake capture.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::Local;

use crate::meeting::{self, Mark, Meeting, MeetingError};
use crate::recording::{Track, TrackStats};
use crate::session::{Session, SessionError};

pub type BoxError = Box<dyn Error + Send + Sync>;

/// What the recorder needs from the platform capture.
pub trait CaptureControl: Sized {
    type Error: Error + Send + Sync + 'static;

    /// Seconds on the clock that the audio times use.
    fn now(&self) -> f64;
    fn sources(&self) -> Sources;
    /// Total size of the video files so far.
    fn video_bytes(&self) -> Option<u64>;
    fn pause_video(&mut self) -> Result<(), Self::Error>;
    /// Continues the video in a new file.
    fn resume_video(&mut self, path: &Path) -> Result<(), Self::Error>;
    /// Stops and waits until the video file is complete.
    fn stop(self) -> Result<(), Self::Error>;
    /// Joins video parts into one file, each placed at its start in seconds.
    fn join_videos(parts: &[(PathBuf, f64)], output: &Path) -> Result<(), Self::Error>;
}

/// What the capture records, after its defaults and lookups.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sources {
    pub microphone: String,
    pub system_audio: String,
    pub video: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    /// The capture could not start. The error is the capture's own.
    #[error("{0}")]
    Capture(BoxError),
    #[error("{context}: {source}")]
    Video {
        context: &'static str,
        source: BoxError,
    },
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Meeting(#[from] MeetingError),
    #[error("the recording is already closed")]
    Closed,
}

/// Takes the audio from the capture threads and writes it to the session.
pub struct Sink {
    session: Mutex<Option<Session>>,
    error: Mutex<Option<String>>,
    /// The loudest sample of each track since the last [`Recorder::status`], as `f32` bits.
    peaks: [AtomicU32; 2],
}

impl Sink {
    pub fn write(&self, track: Track, time: f64, sample_rate: u32, samples: &[f32]) {
        let peak = samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        // For floats that are not negative, the bit patterns sort like the values.
        self.peaks[peak_index(track)].fetch_max(peak.to_bits(), Ordering::Relaxed);
        let written = self.with_session(|session| session.audio(track, time, sample_rate, samples));
        if let Some(Err(error)) = written {
            self.fail(error.to_string());
        }
    }

    /// Keeps the first failure; the recording should then stop.
    pub fn fail(&self, message: String) {
        lock(&self.error).get_or_insert(message);
    }

    fn with_session<T>(&self, action: impl FnOnce(&mut Session) -> T) -> Option<T> {
        lock(&self.session).as_mut().map(action)
    }

    fn take_peak(&self, track: Track) -> f32 {
        f32::from_bits(self.peaks[peak_index(track)].swap(0, Ordering::Relaxed))
    }
}

fn peak_index(track: Track) -> usize {
    match track {
        Track::Me => 0,
        Track::Them => 1,
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    /// Recorded time in seconds, without the pauses.
    pub position: f64,
    pub paused: bool,
    pub marks: usize,
    pub me: TrackStats,
    pub them: TrackStats,
    pub video_bytes: Option<u64>,
    /// The loudest sample of each track since the last call, from 0 to 1.
    pub me_peak: f32,
    pub them_peak: f32,
    /// Why the capture failed. The recording should be stopped then.
    pub error: Option<String>,
}

/// What [`Recorder::stop`] leaves.
#[derive(Debug)]
pub struct Stopped {
    pub folder: PathBuf,
    pub meeting: Meeting,
    /// The video parts could not be joined; they are kept and listed in `meeting.json`.
    pub video_error: Option<MeetingError>,
    /// The capture did not stop cleanly; the files are saved anyway.
    pub capture_error: Option<BoxError>,
}

pub struct Recorder<C: CaptureControl> {
    folder: PathBuf,
    sink: Arc<Sink>,
    capture: C,
    sources: Sources,
}

impl<C: CaptureControl> Recorder<C> {
    /// Creates the session in `folder`, a new folder for this meeting, and starts
    /// the capture with `start`. It gets the first video file (when the meeting has
    /// video) and the sink for the audio. On failure an empty `folder` is removed.
    pub fn start(
        folder: &Path,
        meeting: Meeting,
        start: impl FnOnce(Option<PathBuf>, Arc<Sink>) -> Result<C, C::Error>,
    ) -> Result<Self, RecorderError> {
        let session = match Session::create(folder, meeting) {
            Ok(session) => session,
            Err(error) => {
                let _ = fs::remove_dir(folder);
                return Err(error.into());
            }
        };
        let video_part = session.video_part();
        let sink = Arc::new(Sink {
            session: Mutex::new(Some(session)),
            error: Mutex::new(None),
            peaks: Default::default(),
        });
        let capture = match start(video_part, Arc::clone(&sink)) {
            Ok(capture) => capture,
            Err(error) => {
                if let Some(session) = lock(&sink.session).take() {
                    session.discard();
                }
                let _ = fs::remove_dir(folder);
                return Err(RecorderError::Capture(Box::new(error)));
            }
        };

        let sources = capture.sources();
        let started_at = Local::now().fixed_offset();
        let saved = sink.with_session(|session| {
            session.update(|meeting| {
                meeting.recording.started_at = started_at;
                meeting.tracks.me.source = sources.microphone.clone();
                meeting.tracks.them.source = sources.system_audio.clone();
                if let (Some(video), Some(source)) = (&mut meeting.video, &sources.video) {
                    video.source = source.clone();
                }
            })
        });
        if let Some(Err(error)) = saved {
            let _ = capture.stop();
            return Err(error.into());
        }
        Ok(Self {
            folder: folder.to_path_buf(),
            sink,
            capture,
            sources,
        })
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn sources(&self) -> &Sources {
        &self.sources
    }

    /// Also resets the peaks, so call it from one place.
    pub fn status(&self) -> Status {
        let at = self.capture.now();
        let (position, paused, marks, me, them) = self
            .sink
            .with_session(|session| {
                (
                    session.position(at),
                    session.is_paused(),
                    session.meeting().marks.len(),
                    session.stats(Track::Me).clone(),
                    session.stats(Track::Them).clone(),
                )
            })
            .unwrap_or_default();
        Status {
            position,
            paused,
            marks,
            me,
            them,
            video_bytes: self.capture.video_bytes(),
            me_peak: self.sink.take_peak(Track::Me),
            them_peak: self.sink.take_peak(Track::Them),
            error: lock(&self.sink.error).clone(),
        }
    }

    /// Pauses, or resumes when paused. Returns whether it is paused now.
    pub fn toggle_pause(&mut self) -> Result<bool, RecorderError> {
        let at = self.capture.now();
        let mut guard = lock(&self.sink.session);
        let session = guard.as_mut().ok_or(RecorderError::Closed)?;
        if session.is_paused() {
            let next = session.resume(at)?;
            drop(guard);
            if let Some(path) = next {
                self.capture
                    .resume_video(&path)
                    .map_err(|error| RecorderError::Video {
                        context: "the audio resumed, but the video could not",
                        source: Box::new(error),
                    })?;
            }
            Ok(false)
        } else {
            session.pause(at);
            drop(guard);
            self.capture
                .pause_video()
                .map_err(|error| RecorderError::Video {
                    context: "the audio is paused, but the video could not be",
                    source: Box::new(error),
                })?;
            Ok(true)
        }
    }

    pub fn mark(&self) -> Result<Mark, RecorderError> {
        let at = self.capture.now();
        let mark = self
            .sink
            .with_session(|session| session.mark(at))
            .ok_or(RecorderError::Closed)??;
        Ok(mark)
    }

    /// Stops the capture, closes the files and saves `meeting.json`. The video
    /// parts are joined into one file.
    pub fn stop(self) -> Result<Stopped, RecorderError> {
        let at = self.capture.now();
        let stopped = self.capture.stop();
        let error = lock(&self.sink.error).clone();
        let session = lock(&self.sink.session)
            .take()
            .ok_or(RecorderError::Closed)?;
        let mut meeting = session.finish(at, error)?;
        let video = meeting::finish_video(&self.folder, &mut meeting, C::join_videos);
        meeting.save(&self.folder)?;
        Ok(Stopped {
            folder: self.folder,
            meeting,
            video_error: video.err(),
            capture_error: stopped.err().map(|error| Box::new(error) as BoxError),
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::meeting::{MEETING_FILE, Settings, VIDEO_FILE, VideoFile};
    use crate::recording::SAMPLE_RATE;
    use crate::testing::temp_folder;

    #[derive(Debug, thiserror::Error)]
    #[error("fake capture failed")]
    struct FakeError;

    /// Writes empty files where the real capture would write video.
    struct FakeCapture {
        clock: Arc<Mutex<f64>>,
        video: Option<PathBuf>,
        stopped: Arc<AtomicBool>,
    }

    impl FakeCapture {
        fn start(video: Option<PathBuf>) -> Self {
            if let Some(path) = &video {
                fs::write(path, b"part").expect("write video part");
            }
            Self {
                clock: Arc::new(Mutex::new(0.0)),
                video,
                stopped: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl CaptureControl for FakeCapture {
        type Error = FakeError;

        fn now(&self) -> f64 {
            *lock(&self.clock)
        }

        fn sources(&self) -> Sources {
            Sources {
                microphone: "Test microphone".into(),
                system_audio: "All apps".into(),
                video: self.video.as_ref().map(|_| "Display 1".into()),
            }
        }

        fn video_bytes(&self) -> Option<u64> {
            self.video.as_ref().map(|_| 4)
        }

        fn pause_video(&mut self) -> Result<(), FakeError> {
            Ok(())
        }

        fn resume_video(&mut self, path: &Path) -> Result<(), FakeError> {
            fs::write(path, b"part").map_err(|_| FakeError)
        }

        fn stop(self) -> Result<(), FakeError> {
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn join_videos(_parts: &[(PathBuf, f64)], output: &Path) -> Result<(), FakeError> {
            fs::write(output, b"joined").map_err(|_| FakeError)
        }
    }

    fn meeting(video: bool) -> Meeting {
        let mut meeting = Meeting::new(
            "Sync".into(),
            Local::now().fixed_offset(),
            Settings::default(),
        );
        meeting.video = video.then(|| VideoFile {
            file: VIDEO_FILE.into(),
            source: String::new(),
            fps: 5,
            bytes: None,
            parts: Vec::new(),
        });
        meeting
    }

    /// Feeds both tracks with 10 ms chunks up to `to` seconds and moves the clock there.
    fn feed(sink: &Sink, clock: &Mutex<f64>, to: f64) {
        let mut time = *lock(clock);
        while time < to - 1e-9 {
            for track in [Track::Me, Track::Them] {
                sink.write(track, time, SAMPLE_RATE, &[0.25; 480]);
            }
            time += 0.01;
        }
        *lock(clock) = to;
    }

    fn start(folder: &Path, video: bool) -> (Recorder<FakeCapture>, Arc<Sink>, Arc<Mutex<f64>>) {
        let mut parts = None;
        let recorder = Recorder::start(folder, meeting(video), |video, sink| {
            let capture = FakeCapture::start(video);
            parts = Some((sink, Arc::clone(&capture.clock)));
            Ok(capture)
        })
        .expect("start");
        let (sink, clock) = parts.expect("capture started");
        (recorder, sink, clock)
    }

    #[test]
    fn records_pauses_and_marks_into_the_meeting() {
        let folder = temp_folder("recorder-full");
        let (mut recorder, sink, clock) = start(&folder, true);
        feed(&sink, &clock, 2.0);
        assert!(recorder.toggle_pause().expect("pause"));
        *lock(&clock) = 5.0;
        assert!(!recorder.toggle_pause().expect("resume"));
        feed(&sink, &clock, 6.0);
        let mark = recorder.mark().expect("mark");
        assert!(
            (mark.at_seconds - 3.0).abs() < 0.02,
            "mark at {}",
            mark.at_seconds
        );

        let status = recorder.status();
        assert!((status.position - 3.0).abs() < 0.02);
        assert_eq!(status.marks, 1);
        assert!(!status.paused);

        let stopped = recorder.stop().expect("stop");
        assert!(stopped.video_error.is_none());
        assert!(stopped.capture_error.is_none());
        let saved = Meeting::load(&folder).expect("load");
        assert_eq!(saved, stopped.meeting);
        assert!((saved.recording.duration_seconds - 3.0).abs() < 0.02);
        assert_eq!(saved.pauses.len(), 1);
        assert_eq!(saved.tracks.me.source, "Test microphone");
        let video = saved.video.expect("video");
        assert_eq!(video.source, "Display 1");
        assert!(video.parts.is_empty());
        assert_eq!(fs::read(folder.join(VIDEO_FILE)).expect("video"), b"joined");
        assert!(folder.join(Track::Me.file_name()).exists());
    }

    #[test]
    fn status_reports_the_peak_once() {
        let folder = temp_folder("recorder-peaks");
        let (recorder, sink, clock) = start(&folder, false);
        feed(&sink, &clock, 0.5);
        let status = recorder.status();
        assert_eq!(status.me_peak, 0.25);
        assert_eq!(status.them_peak, 0.25);
        assert_eq!(recorder.status().me_peak, 0.0);
        recorder.stop().expect("stop");
    }

    #[test]
    fn a_capture_failure_is_kept_for_status_and_the_meeting() {
        let folder = temp_folder("recorder-failure");
        let (recorder, sink, clock) = start(&folder, false);
        feed(&sink, &clock, 0.5);
        sink.fail("the microphone was disconnected".into());
        sink.fail("a later error".into());
        assert_eq!(
            recorder.status().error.as_deref(),
            Some("the microphone was disconnected")
        );
        let stopped = recorder.stop().expect("stop");
        assert_eq!(
            stopped.meeting.recording.error.as_deref(),
            Some("the microphone was disconnected")
        );
    }

    #[test]
    fn a_capture_that_does_not_start_leaves_no_folder() {
        let library = temp_folder("recorder-no-start");
        let folder = library.join("meeting");
        fs::create_dir(&folder).expect("create folder");
        let result = Recorder::<FakeCapture>::start(&folder, meeting(true), |_, _| Err(FakeError));
        assert!(matches!(result, Err(RecorderError::Capture(_))));
        assert!(!folder.exists());
        assert!(!library.join(MEETING_FILE).exists());
    }
}
