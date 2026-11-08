//! One recording in progress: the tracks, pauses, marks and `meeting.json`.
//! Times passed in are seconds on the capture clock, the clock of the audio
//! timestamps.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::meeting::{
    MEETING_FILE, Mark, Meeting, MeetingError, Pause, VideoPart, video_part_name,
};
use crate::recording::{RecordingError, Track, TrackStats, Tracks};

/// Held while recording. The OS releases it when the process ends, so a folder
/// whose lock can be taken and whose recording never stopped was cut off.
pub const LOCK_FILE: &str = ".recording.lock";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("{} is already being recorded", path.display())]
    AlreadyRecording { path: PathBuf },
    #[error("could not lock {}: {source}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Recording(#[from] RecordingError),
    #[error(transparent)]
    Meeting(#[from] MeetingError),
}

pub struct Session {
    folder: PathBuf,
    meeting: Meeting,
    tracks: Tracks,
    timeline: Timeline,
    lock: File,
}

impl Session {
    /// Takes the folder's lock, creates the WAV files and writes `meeting.json`.
    /// With video on, the first part's file is [`Session::video_part`].
    pub fn create(folder: &Path, mut meeting: Meeting) -> Result<Self, SessionError> {
        let lock = take_lock(folder)?;
        let tracks = Tracks::create(folder)?;
        if let Some(video) = &mut meeting.video {
            video.parts = vec![VideoPart {
                file: video_part_name(1),
                at_seconds: 0.0,
            }];
        }
        meeting.save(folder)?;
        Ok(Self {
            folder: folder.to_path_buf(),
            meeting,
            tracks,
            timeline: Timeline::default(),
            lock,
        })
    }

    pub fn meeting(&self) -> &Meeting {
        &self.meeting
    }

    pub fn update(&mut self, change: impl FnOnce(&mut Meeting)) -> Result<(), MeetingError> {
        change(&mut self.meeting);
        self.meeting.save(&self.folder)
    }

    /// The file the video is being written to, or will be after a resume.
    pub fn video_part(&self) -> Option<PathBuf> {
        let video = self.meeting.video.as_ref()?;
        video.parts.last().map(|part| self.folder.join(&part.file))
    }

    pub fn audio(
        &mut self,
        track: Track,
        time: f64,
        sample_rate: u32,
        samples: &[f32],
    ) -> Result<(), RecordingError> {
        match self.timeline.position(time) {
            Some(offset) => self.tracks.write(track, offset, sample_rate, samples),
            None => Ok(()),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.timeline.paused_since().is_some()
    }

    /// How much audio has been recorded by `at`.
    pub fn position(&self, at: f64) -> f64 {
        self.timeline.position_at(at)
    }

    pub fn stats(&self, track: Track) -> &TrackStats {
        self.tracks.stats(track)
    }

    pub fn pause(&mut self, at: f64) {
        self.timeline.pause(at);
    }

    /// Returns the file for the next video part when video is on.
    pub fn resume(&mut self, at: f64) -> Result<Option<PathBuf>, MeetingError> {
        let Some(since) = self.timeline.paused_since() else {
            return Ok(None);
        };
        let position = self.timeline.position_at(since);
        self.timeline.resume(at);
        self.meeting.pauses.push(Pause {
            at_seconds: position,
            duration_seconds: (at - since).max(0.0),
        });
        if let Some(video) = &mut self.meeting.video {
            video.parts.push(VideoPart {
                file: video_part_name(video.parts.len() + 1),
                at_seconds: position,
            });
        }
        self.meeting.save(&self.folder)?;
        Ok(self.video_part())
    }

    pub fn mark(&mut self, at: f64) -> Result<Mark, MeetingError> {
        let mark = Mark {
            at_seconds: self.timeline.position_at(at),
        };
        self.meeting.marks.push(mark);
        self.meeting.save(&self.folder)?;
        Ok(mark)
    }

    /// Removes what [`Session::create`] wrote, for a recording that never started.
    pub fn discard(self) {
        drop(self.tracks);
        for file in [
            Track::Me.file_name(),
            Track::Them.file_name(),
            MEETING_FILE,
            LOCK_FILE,
        ] {
            let _ = fs::remove_file(self.folder.join(file));
        }
    }

    /// Closes the WAV files, saves `meeting.json` and releases the lock. The
    /// video parts are left for [`crate::meeting::finish_video`].
    pub fn finish(mut self, at: f64, error: Option<String>) -> Result<Meeting, SessionError> {
        if let Some(since) = self.timeline.paused_since() {
            self.meeting.pauses.push(Pause {
                at_seconds: self.timeline.position_at(since),
                duration_seconds: (at - since).max(0.0),
            });
        }
        let stats = self.tracks.finish()?;
        let recording = &mut self.meeting.recording;
        recording.stopped_at = Some(Local::now().fixed_offset());
        recording.duration_seconds = self.timeline.position_at(at);
        recording.error = error;
        self.meeting.tracks.me.stats = Some(stats.me);
        self.meeting.tracks.them.stats = Some(stats.them);
        self.meeting.save(&self.folder)?;
        drop(self.lock);
        let _ = fs::remove_file(self.folder.join(LOCK_FILE));
        Ok(self.meeting)
    }
}

pub fn is_recording(folder: &Path) -> bool {
    match File::open(folder.join(LOCK_FILE)) {
        Ok(file) => matches!(file.try_lock(), Err(TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

fn take_lock(folder: &Path) -> Result<File, SessionError> {
    let path = folder.join(LOCK_FILE);
    let lock_error = |source| SessionError::Lock {
        path: path.clone(),
        source,
    };
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(lock_error)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(SessionError::AlreadyRecording {
            path: folder.to_path_buf(),
        }),
        Err(TryLockError::Error(source)) => Err(lock_error(source)),
    }
}

/// Maps capture time to a position in the recorded audio: the origin is the
/// first chunk from either track, and paused time is cut out.
#[derive(Debug, Default)]
struct Timeline {
    origin: Option<f64>,
    cuts: Vec<Cut>,
}

#[derive(Debug, Clone, Copy)]
struct Cut {
    start: f64,
    end: Option<f64>,
}

impl Timeline {
    /// `None` when `time` falls into a pause.
    fn position(&mut self, time: f64) -> Option<f64> {
        let origin = *self.origin.get_or_insert(time);
        let mut cut = 0.0;
        for c in &self.cuts {
            if time < c.start {
                break;
            }
            match c.end {
                Some(end) if time >= end => cut += (end - c.start.max(origin)).max(0.0),
                _ => return None,
            }
        }
        Some(time - origin - cut)
    }

    /// Like `position`, but a time inside a pause gives the pause's position.
    fn position_at(&self, time: f64) -> f64 {
        let Some(origin) = self.origin else {
            return 0.0;
        };
        let mut time = time;
        let mut cut = 0.0;
        for c in &self.cuts {
            if time < c.start {
                break;
            }
            match c.end {
                Some(end) if time >= end => cut += (end - c.start.max(origin)).max(0.0),
                _ => {
                    time = c.start;
                    break;
                }
            }
        }
        (time - origin - cut).max(0.0)
    }

    fn paused_since(&self) -> Option<f64> {
        self.cuts
            .last()
            .filter(|cut| cut.end.is_none())
            .map(|cut| cut.start)
    }

    fn pause(&mut self, at: f64) {
        if self.paused_since().is_none() {
            let start = self
                .cuts
                .last()
                .and_then(|cut| cut.end)
                .map_or(at, |end| at.max(end));
            self.cuts.push(Cut { start, end: None });
        }
    }

    fn resume(&mut self, at: f64) {
        if let Some(cut) = self.cuts.last_mut().filter(|cut| cut.end.is_none()) {
            cut.end = Some(at.max(cut.start));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{Settings, VIDEO_FILE, VideoFile};
    use crate::recording::SAMPLE_RATE;
    use crate::testing::temp_folder;

    const CHUNK: usize = 480;
    const CHUNK_SECONDS: f64 = 0.01;

    fn meeting(video: bool) -> Meeting {
        let started_at =
            chrono::DateTime::parse_from_rfc3339("2026-11-04T14:30:00+03:00").expect("valid time");
        let mut meeting = Meeting::new("Sync".into(), started_at, Settings::default());
        if video {
            meeting.video = Some(VideoFile {
                file: VIDEO_FILE.into(),
                source: "Display 1".into(),
                fps: 5,
                bytes: None,
                parts: Vec::new(),
            });
        }
        meeting
    }

    /// Feeds both tracks with 10 ms chunks from `from` to `to` on the capture clock.
    fn feed(session: &mut Session, from: f64, to: f64) {
        let mut time = from;
        while time < to - 1e-9 {
            for track in [Track::Me, Track::Them] {
                session
                    .audio(track, time, SAMPLE_RATE, &[0.5; CHUNK])
                    .expect("write");
            }
            time += CHUNK_SECONDS;
        }
    }

    #[test]
    fn the_origin_is_the_first_chunk_from_either_track() {
        let mut timeline = Timeline::default();
        assert_eq!(timeline.position(100.0), Some(0.0));
        assert_eq!(timeline.position(100.25), Some(0.25));
        assert_eq!(
            timeline.position(99.9).map(|p| (p * 10.0).round()),
            Some(-1.0)
        );
    }

    #[test]
    fn paused_time_is_cut_out() {
        let mut timeline = Timeline::default();
        timeline.position(100.0);
        timeline.pause(110.0);
        assert_eq!(timeline.position(112.0), None);
        assert_eq!(timeline.position_at(112.0), 10.0);
        timeline.resume(130.0);
        assert_eq!(timeline.position(130.0), Some(10.0));
        assert_eq!(timeline.position(135.0), Some(15.0));
        timeline.pause(140.0);
        timeline.resume(141.0);
        assert_eq!(timeline.position_at(150.0), 29.0);
    }

    #[test]
    fn audio_from_before_the_pause_still_counts_when_it_arrives_late() {
        let mut timeline = Timeline::default();
        timeline.position(100.0);
        timeline.pause(110.0);
        assert_eq!(
            timeline.position(109.99).map(|p| (p * 100.0).round()),
            Some(999.0)
        );
    }

    #[test]
    fn a_pause_before_any_audio_costs_nothing() {
        let mut timeline = Timeline::default();
        timeline.pause(100.0);
        timeline.resume(105.0);
        assert_eq!(timeline.position(105.5), Some(0.0));
        assert_eq!(timeline.position_at(106.5), 1.0);
    }

    #[test]
    fn records_pauses_marks_and_video_parts() {
        let folder = temp_folder("session-pause");
        let mut session = Session::create(&folder, meeting(true)).expect("create");
        assert_eq!(session.video_part(), Some(folder.join("screen-1.mp4")));

        feed(&mut session, 100.0, 102.0);
        session.mark(101.5).expect("mark");
        session.pause(102.0);
        assert!(session.is_paused());
        feed(&mut session, 102.0, 105.0);
        let next = session.resume(105.0).expect("resume");
        assert_eq!(next, Some(folder.join("screen-2.mp4")));
        feed(&mut session, 105.0, 106.0);
        session.mark(105.5).expect("mark");

        let saved = Meeting::load(&folder).expect("load");
        assert_eq!(saved.marks.len(), 2);
        assert_eq!(saved.pauses.len(), 1);

        let meeting = session.finish(106.0, None).expect("finish");
        let marks: Vec<f64> = meeting.marks.iter().map(|m| m.at_seconds).collect();
        assert_eq!(marks, vec![1.5, 2.5]);
        assert_eq!(meeting.pauses[0].at_seconds, 2.0);
        assert_eq!(meeting.pauses[0].duration_seconds, 3.0);
        assert!((meeting.recording.duration_seconds - 3.0).abs() < 1e-9);
        let parts = &meeting.video.as_ref().expect("video").parts;
        assert_eq!(parts[1].at_seconds, 2.0);
        assert!(meeting.recording.stopped_at.is_some());

        for stats in [&meeting.tracks.me.stats, &meeting.tracks.them.stats] {
            let stats = stats.as_ref().expect("stats");
            assert_eq!(stats.frames, 3 * SAMPLE_RATE as u64);
            assert_eq!(stats.gaps_filled, 0);
        }
        assert!(!folder.join(LOCK_FILE).exists());
    }

    #[test]
    fn stopping_while_paused_closes_the_pause() {
        let folder = temp_folder("session-stop-paused");
        let mut session = Session::create(&folder, meeting(false)).expect("create");
        feed(&mut session, 50.0, 51.0);
        session.pause(51.0);
        let meeting = session.finish(54.0, None).expect("finish");
        assert_eq!(meeting.pauses.len(), 1);
        assert_eq!(meeting.pauses[0].duration_seconds, 3.0);
        assert!((meeting.recording.duration_seconds - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_folder_is_locked_while_recording() {
        let folder = temp_folder("session-lock");
        let session = Session::create(&folder, meeting(false)).expect("create");
        assert!(is_recording(&folder));
        assert!(folder.join(MEETING_FILE).exists());
        let second = Session::create(&folder, meeting(false));
        assert!(matches!(second, Err(SessionError::AlreadyRecording { .. })));
        session.finish(0.0, None).expect("finish");
        assert!(!is_recording(&folder));
    }
}
