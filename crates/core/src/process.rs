//! The processing chain. It works through the steps in `meeting.json` and saves
//! each step's status, so after a crash or quit it continues where it stopped.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use chrono::Local;

use crate::audio::{AudioError, WHISPER_RATE, read_for_whisper};
use crate::export::{ExportError, export};
use crate::files::write_atomically;
use crate::meeting::{Language, Meeting, MeetingError, Step, StepState, StepStatus};
use crate::recording::Track;
use crate::session::is_recording;
use crate::summary::{self, PromptError, SummarizeError, Summarizer, TooLong, WriteError};
use crate::transcript::{TrackText, Transcript, TranscriptError};

/// Each track's transcript waits here until both are merged.
pub const WORK_FOLDER: &str = ".work";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Failed(Box<dyn Error + Send + Sync>),
}

/// Speech to text for one track. The chain runs it on a worker thread.
pub trait Transcriber: Send {
    /// Saved with the step, for example `large-v3-turbo`.
    fn model(&self) -> &str;

    /// `audio` is 16 kHz mono and not empty. Report progress and check for
    /// cancelling through `progress`.
    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Language,
        progress: &Progress,
    ) -> Result<TrackText, TranscribeError>;
}

/// Shared with the transcriber's callbacks, which may need owned handles.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    done: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
}

impl Progress {
    pub fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            done: Arc::default(),
            cancelled,
        }
    }

    pub fn set(&self, fraction: f32) {
        let value = (fraction.clamp(0.0, 1.0) * 10_000.0) as u32;
        self.done.store(value, Ordering::Relaxed);
    }

    pub fn fraction(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / 10_000.0
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn cancelled_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Started(Step),
    /// `done` goes from 0 to 1 for each track.
    Transcribing {
        track: Track,
        done: f32,
    },
    /// Characters of the summary received so far.
    Summarizing {
        characters: usize,
    },
    Finished(Step),
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("{} is still being recorded", path.display())]
    StillRecording { path: PathBuf },
    #[error("the recording in {} was cut off; repair it first", path.display())]
    NeedsRepair { path: PathBuf },
    #[error("processing was cancelled")]
    Cancelled,
    #[error(transparent)]
    Meeting(#[from] MeetingError),
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Transcript(#[from] TranscriptError),
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error("transcribing failed: {0}")]
    Transcribe(#[source] Box<dyn Error + Send + Sync>),
    #[error("{} has no transcript yet; process it first", path.display())]
    NotTranscribed { path: PathBuf },
    #[error("summaries are not set up: choose a provider in Settings")]
    NoSummaryProvider,
    #[error(transparent)]
    TooLong(#[from] TooLong),
    #[error("writing the summary failed: {0}")]
    Summarize(#[source] Box<dyn Error + Send + Sync>),
    #[error(transparent)]
    SummaryFile(#[from] WriteError),
    #[error(transparent)]
    Prompt(#[from] PromptError),
}

/// Transcribes, merges and writes the transcript files, and returns the updated
/// meeting. Steps already done are skipped. The summary is written only on
/// request, by [`summarize_now`].
pub fn process(
    folder: &Path,
    transcriber: &mut dyn Transcriber,
    cancelled: &Arc<AtomicBool>,
    on_event: &mut dyn FnMut(Event),
) -> Result<Meeting, ProcessError> {
    let mut meeting = load_stopped(folder)?;
    while let Some(step) = meeting.next_step() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(ProcessError::Cancelled);
        }
        match step {
            Step::Transcribe => {
                let model = transcriber.model().to_string();
                run_step(
                    &mut meeting,
                    folder,
                    step,
                    Some(model),
                    on_event,
                    |meeting, on_event| {
                        transcribe_tracks(folder, meeting, transcriber, cancelled, on_event)
                    },
                )?;
            }
            Step::Merge => run_step(&mut meeting, folder, step, None, on_event, |_, _| {
                merge(folder)
            })?,
            Step::Export => run_step(&mut meeting, folder, step, None, on_event, |meeting, _| {
                export_files(folder, meeting)
            })?,
            Step::Summary | Step::Compress => break,
        }
    }
    Ok(meeting)
}

/// Writes or rewrites the summary of a transcribed meeting, with the meeting's
/// own prompt if it has one.
pub fn summarize_now(
    folder: &Path,
    summarizer: &mut dyn Summarizer,
    cancelled: &Arc<AtomicBool>,
    on_event: &mut dyn FnMut(Event),
) -> Result<Meeting, ProcessError> {
    let mut meeting = load_stopped(folder)?;
    if meeting.status(Step::Merge) != StepStatus::Done {
        return Err(ProcessError::NotTranscribed {
            path: folder.to_path_buf(),
        });
    }
    let prompt = match summary::own_prompt(folder)? {
        Some(prompt) => prompt,
        None => summarizer.prompt().to_string(),
    };
    let model = summarizer.model().to_string();
    run_step(
        &mut meeting,
        folder,
        Step::Summary,
        Some(model),
        on_event,
        |meeting, on_event| {
            write_summary(folder, meeting, summarizer, &prompt, cancelled, on_event)
        },
    )?;
    Ok(meeting)
}

fn load_stopped(folder: &Path) -> Result<Meeting, ProcessError> {
    if is_recording(folder) {
        return Err(ProcessError::StillRecording {
            path: folder.to_path_buf(),
        });
    }
    let meeting = Meeting::load(folder)?;
    if meeting.recording.stopped_at.is_none() {
        return Err(ProcessError::NeedsRepair {
            path: folder.to_path_buf(),
        });
    }
    Ok(meeting)
}

/// Saves the step as running, runs it, and saves how it ended. A cancelled step
/// goes back to pending.
fn run_step(
    meeting: &mut Meeting,
    folder: &Path,
    step: Step,
    model: Option<String>,
    on_event: &mut dyn FnMut(Event),
    run: impl FnOnce(&Meeting, &mut dyn FnMut(Event)) -> Result<(), ProcessError>,
) -> Result<(), ProcessError> {
    set_step(
        meeting,
        folder,
        step,
        StepStatus::Running,
        model.clone(),
        None,
    )?;
    on_event(Event::Started(step));
    match run(meeting, on_event) {
        Ok(()) => {
            set_step(meeting, folder, step, StepStatus::Done, model, None)?;
            on_event(Event::Finished(step));
            Ok(())
        }
        Err(ProcessError::Cancelled) => {
            meeting.steps.remove(&step);
            meeting.save(folder)?;
            Err(ProcessError::Cancelled)
        }
        Err(error) => {
            let message = error.to_string();
            set_step(
                meeting,
                folder,
                step,
                StepStatus::Failed,
                model,
                Some(message),
            )?;
            Err(error)
        }
    }
}

fn set_step(
    meeting: &mut Meeting,
    folder: &Path,
    step: Step,
    status: StepStatus,
    model: Option<String>,
    error: Option<String>,
) -> Result<(), MeetingError> {
    let now = Local::now().fixed_offset();
    let started_at = match status {
        StepStatus::Running => Some(now),
        _ => meeting.steps.get(&step).and_then(|state| state.started_at),
    };
    let finished_at = matches!(status, StepStatus::Done | StepStatus::Failed).then_some(now);
    meeting.steps.insert(
        step,
        StepState {
            status,
            started_at,
            finished_at,
            error,
            model,
        },
    );
    meeting.save(folder)
}

fn work_file(folder: &Path, track: Track) -> PathBuf {
    let name = match track {
        Track::Me => "me.json",
        Track::Them => "them.json",
    };
    folder.join(WORK_FOLDER).join(name)
}

fn transcribe_tracks(
    folder: &Path,
    meeting: &Meeting,
    transcriber: &mut dyn Transcriber,
    cancelled: &Arc<AtomicBool>,
    on_event: &mut dyn FnMut(Event),
) -> Result<(), ProcessError> {
    let language = meeting.settings.language;
    for (track, audio_track) in [
        (Track::Me, &meeting.tracks.me),
        (Track::Them, &meeting.tracks.them),
    ] {
        let path = work_file(folder, track);
        if read_track_text(&path).is_ok() {
            on_event(Event::Transcribing { track, done: 1.0 });
            continue;
        }
        let audio = read_for_whisper(&folder.join(&audio_track.file))?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(ProcessError::Cancelled);
        }
        let text = if audio.is_empty() {
            TrackText {
                language: match language {
                    Language::Auto => String::new(),
                    fixed => fixed.code().to_string(),
                },
                lines: Vec::new(),
            }
        } else {
            let text = run_transcriber(transcriber, &audio, language, cancelled, track, on_event)?;
            within_audio(text, audio.len() as f64 / f64::from(WHISPER_RATE))
        };
        write_track_text(&path, &text)?;
        on_event(Event::Transcribing { track, done: 1.0 });
    }
    Ok(())
}

fn run_transcriber(
    transcriber: &mut dyn Transcriber,
    audio: &[f32],
    language: Language,
    cancelled: &Arc<AtomicBool>,
    track: Track,
    on_event: &mut dyn FnMut(Event),
) -> Result<TrackText, ProcessError> {
    let progress = Progress::new(Arc::clone(cancelled));
    let outcome = thread::scope(|scope| {
        let worker_progress = &progress;
        let worker = scope.spawn(move || transcriber.transcribe(audio, language, worker_progress));
        while !worker.is_finished() {
            on_event(Event::Transcribing {
                track,
                done: progress.fraction(),
            });
            thread::sleep(PROGRESS_INTERVAL);
        }
        worker.join()
    });
    match outcome {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(TranscribeError::Cancelled)) => Err(ProcessError::Cancelled),
        Ok(Err(TranscribeError::Failed(error))) => Err(ProcessError::Transcribe(error)),
        Err(_) => Err(ProcessError::Transcribe("the transcriber crashed".into())),
    }
}

/// Whisper can place a line past the end of the audio, from the padding of its
/// last 30-second window. Such times are impossible, so they are cut to the end.
fn within_audio(mut text: TrackText, seconds: f64) -> TrackText {
    text.lines.retain(|line| line.start < seconds);
    for line in &mut text.lines {
        line.end = line.end.min(seconds);
    }
    text
}

fn merge(folder: &Path) -> Result<(), ProcessError> {
    let me = read_track_text(&work_file(folder, Track::Me))?;
    let them = read_track_text(&work_file(folder, Track::Them))?;
    Transcript::merge(me, them).save(folder)?;
    let _ = fs::remove_dir_all(folder.join(WORK_FOLDER));
    Ok(())
}

fn write_summary(
    folder: &Path,
    meeting: &Meeting,
    summarizer: &mut dyn Summarizer,
    prompt: &str,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Event),
) -> Result<(), ProcessError> {
    let transcript = Transcript::load(folder)?;
    let text = summary::input_text(meeting, &transcript);
    summary::check_fits(summarizer, prompt, &text)?;
    let summary = summarizer
        .summarize(
            prompt,
            &text,
            &mut |characters| on_event(Event::Summarizing { characters }),
            cancelled,
        )
        .map_err(|error| match error {
            SummarizeError::Cancelled => ProcessError::Cancelled,
            SummarizeError::Failed(error) => ProcessError::Summarize(error),
        })?;
    summary::write_files(folder, meeting, &transcript, &summary)?;
    Ok(())
}

fn export_files(folder: &Path, meeting: &Meeting) -> Result<(), ProcessError> {
    let transcript = Transcript::load(folder)?;
    export(folder, meeting, &transcript)?;
    Ok(())
}

fn read_track_text(path: &Path) -> Result<TrackText, TranscriptError> {
    let text = fs::read_to_string(path).map_err(|source| TranscriptError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| TranscriptError::Invalid {
        path: path.to_path_buf(),
        source,
    })
}

fn write_track_text(path: &Path, text: &TrackText) -> Result<(), TranscriptError> {
    let write_error = |source| TranscriptError::Write {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_error)?;
    }
    let json = serde_json::to_vec(text).map_err(|e| write_error(e.into()))?;
    write_atomically(path, &json).map_err(write_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{MARKDOWN_FILE, SUBTITLES_FILE, TEXT_FILE};
    use crate::meeting::Settings;
    use crate::recording::{SAMPLE_RATE, TrackWriter};
    use crate::testing::temp_folder;
    use crate::transcript::{Line, TRANSCRIPT_FILE};

    #[derive(Default)]
    struct Fake {
        calls: usize,
        cancel_on_call: Option<usize>,
        fail: bool,
    }

    impl Transcriber for Fake {
        fn model(&self) -> &str {
            "fake"
        }

        fn transcribe(
            &mut self,
            audio: &[f32],
            language: Language,
            progress: &Progress,
        ) -> Result<TrackText, TranscribeError> {
            self.calls += 1;
            progress.set(0.5);
            if self.cancel_on_call == Some(self.calls) {
                progress.cancelled_flag().store(true, Ordering::Relaxed);
                return Err(TranscribeError::Cancelled);
            }
            if self.fail {
                return Err(TranscribeError::Failed("model broke".into()));
            }
            Ok(TrackText {
                language: language.code().into(),
                lines: vec![Line {
                    start: 0.25,
                    end: 0.75,
                    text: format!("{} samples", audio.len()),
                }],
            })
        }
    }

    fn recorded_meeting(name: &str) -> PathBuf {
        let folder = temp_folder(name);
        let started_at =
            chrono::DateTime::parse_from_rfc3339("2026-11-04T14:30:00+03:00").expect("valid time");
        let mut meeting = Meeting::new("Sync".into(), started_at, Settings::default());
        meeting.recording.stopped_at = Some(started_at);
        meeting.save(&folder).expect("save");
        for track in [Track::Me, Track::Them] {
            let mut writer = TrackWriter::create(&folder.join(track.file_name())).expect("create");
            writer
                .write(0.0, SAMPLE_RATE, &vec![0.1; SAMPLE_RATE as usize])
                .expect("write");
            writer.finish().expect("finish");
        }
        folder
    }

    fn run(folder: &Path, fake: &mut Fake) -> (Result<Meeting, ProcessError>, Vec<Event>) {
        let mut events = Vec::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let result = process(folder, fake, &cancelled, &mut |event| events.push(event));
        (result, events)
    }

    #[test]
    fn transcribes_both_tracks_and_merges_them() {
        let folder = recorded_meeting("process-full");
        let mut fake = Fake::default();
        let (result, events) = run(&folder, &mut fake);
        let meeting = result.expect("process");

        assert_eq!(fake.calls, 2);
        assert_eq!(meeting.status(Step::Transcribe), StepStatus::Done);
        assert_eq!(meeting.status(Step::Merge), StepStatus::Done);
        assert_eq!(meeting.status(Step::Export), StepStatus::Done);
        assert_eq!(meeting.status(Step::Summary), StepStatus::Pending);
        assert!(!folder.join(summary::MARKDOWN_FILE).exists());
        for file in [MARKDOWN_FILE, TEXT_FILE, SUBTITLES_FILE] {
            assert!(folder.join(file).exists(), "{file}");
        }
        let state = &meeting.steps[&Step::Transcribe];
        assert_eq!(state.model.as_deref(), Some("fake"));
        assert!(state.started_at.is_some() && state.finished_at.is_some());

        let transcript = Transcript::load(&folder).expect("load");
        assert_eq!(transcript.segments.len(), 2);
        assert_eq!(transcript.segments[0].text, "16000 samples");
        assert_eq!(transcript.segments[0].language, "ru");
        assert!(!folder.join(WORK_FOLDER).exists());
        assert!(events.contains(&Event::Started(Step::Transcribe)));
        assert_eq!(events.last(), Some(&Event::Finished(Step::Export)));
        assert_eq!(Meeting::load(&folder).expect("load"), meeting);
    }

    #[test]
    fn resumes_with_the_track_that_was_not_finished() {
        let folder = recorded_meeting("process-resume");
        let me = TrackText {
            language: "ru".into(),
            lines: Vec::new(),
        };
        write_track_text(&work_file(&folder, Track::Me), &me).expect("write");
        let mut meeting = Meeting::load(&folder).expect("load");
        meeting.steps.insert(
            Step::Transcribe,
            StepState {
                status: StepStatus::Running,
                ..StepState::default()
            },
        );
        meeting.save(&folder).expect("save");

        let mut fake = Fake::default();
        let (result, _) = run(&folder, &mut fake);
        result.expect("process");
        assert_eq!(fake.calls, 1);
        let transcript = Transcript::load(&folder).expect("load");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].speaker, Track::Them);
    }

    #[test]
    fn cancelling_keeps_finished_tracks_and_leaves_the_step_to_resume() {
        let folder = recorded_meeting("process-cancel");
        let mut fake = Fake {
            cancel_on_call: Some(2),
            ..Fake::default()
        };
        let (result, _) = run(&folder, &mut fake);
        assert!(matches!(result, Err(ProcessError::Cancelled)));
        let meeting = Meeting::load(&folder).expect("load");
        assert_eq!(meeting.status(Step::Transcribe), StepStatus::Pending);
        assert!(work_file(&folder, Track::Me).exists());
        assert!(!folder.join(TRANSCRIPT_FILE).exists());
    }

    #[test]
    fn a_failure_is_saved_with_the_step() {
        let folder = recorded_meeting("process-fail");
        let mut fake = Fake {
            fail: true,
            ..Fake::default()
        };
        let (result, _) = run(&folder, &mut fake);
        assert!(matches!(result, Err(ProcessError::Transcribe(_))));
        let meeting = Meeting::load(&folder).expect("load");
        let state = &meeting.steps[&Step::Transcribe];
        assert_eq!(state.status, StepStatus::Failed);
        assert_eq!(
            state.error.as_deref(),
            Some("transcribing failed: model broke")
        );
        assert_eq!(meeting.next_step(), Some(Step::Transcribe));
    }

    #[test]
    fn lines_are_cut_to_the_end_of_the_audio() {
        let text = TrackText {
            language: "ru".into(),
            lines: vec![
                Line {
                    start: 87.7,
                    end: 117.6,
                    text: "end".into(),
                },
                Line {
                    start: 95.0,
                    end: 100.0,
                    text: "after".into(),
                },
            ],
        };
        let text = within_audio(text, 90.0);
        assert_eq!(text.lines.len(), 1);
        assert_eq!(text.lines[0].end, 90.0);
    }

    #[test]
    fn a_recording_that_never_stopped_needs_a_repair_first() {
        let folder = recorded_meeting("process-not-stopped");
        let mut meeting = Meeting::load(&folder).expect("load");
        meeting.recording.stopped_at = None;
        meeting.save(&folder).expect("save");
        let (result, _) = run(&folder, &mut Fake::default());
        assert!(matches!(result, Err(ProcessError::NeedsRepair { .. })));
    }

    struct Writer {
        context: usize,
        seen: Option<String>,
        prompt_used: Option<String>,
    }

    impl Summarizer for Writer {
        fn model(&self) -> &str {
            "writer"
        }

        fn prompt(&self) -> &str {
            "Summarize."
        }

        fn context_tokens(&self) -> usize {
            self.context
        }

        fn answer_tokens(&self) -> usize {
            10
        }

        fn summarize(
            &mut self,
            prompt: &str,
            transcript: &str,
            on_text: &mut dyn FnMut(usize),
            _: &AtomicBool,
        ) -> Result<String, SummarizeError> {
            self.prompt_used = Some(prompt.to_string());
            self.seen = Some(transcript.to_string());
            on_text(20);
            Ok("## Summary\n\nAll **good**.".into())
        }
    }

    fn writer() -> Writer {
        Writer {
            context: 100_000,
            seen: None,
            prompt_used: None,
        }
    }

    fn summarize(
        folder: &Path,
        writer: &mut Writer,
    ) -> (Result<Meeting, ProcessError>, Vec<Event>) {
        let mut events = Vec::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let result = summarize_now(folder, writer, &cancelled, &mut |event| events.push(event));
        (result, events)
    }

    #[test]
    fn a_summary_is_written_on_request_with_the_default_prompt() {
        let folder = recorded_meeting("process-summary");
        run(&folder, &mut Fake::default()).0.expect("process");
        let mut writer = writer();
        let (result, events) = summarize(&folder, &mut writer);
        let meeting = result.expect("summarize");
        assert_eq!(meeting.status(Step::Summary), StepStatus::Done);
        assert_eq!(
            meeting.steps[&Step::Summary].model.as_deref(),
            Some("writer")
        );
        assert!(events.contains(&Event::Summarizing { characters: 20 }));
        assert_eq!(writer.prompt_used.as_deref(), Some("Summarize."));
        let seen = writer.seen.expect("summarized");
        assert!(seen.contains("16000 samples"));
        assert!(!seen.contains("left out"));
        let text = fs::read_to_string(folder.join(summary::TEXT_FILE)).expect("read");
        assert!(text.starts_with("Sync\n"));
        assert!(text.ends_with("Summary\n\nAll good.\n"));
        assert!(folder.join(summary::MARKDOWN_FILE).exists());
    }

    #[test]
    fn a_meetings_own_prompt_is_used() {
        let folder = recorded_meeting("process-summary-own-prompt");
        run(&folder, &mut Fake::default()).0.expect("process");
        summary::set_own_prompt(&folder, Some("Only the decisions.")).expect("save");
        let mut writer = writer();
        summarize(&folder, &mut writer).0.expect("summarize");
        assert_eq!(writer.prompt_used.as_deref(), Some("Only the decisions."));
    }

    #[test]
    fn a_transcript_too_long_for_the_model_is_refused() {
        let folder = recorded_meeting("process-summary-long");
        run(&folder, &mut Fake::default()).0.expect("process");
        let mut writer = Writer {
            context: 30,
            ..writer()
        };
        let (result, _) = summarize(&folder, &mut writer);
        assert!(matches!(result, Err(ProcessError::TooLong(_))));
        assert!(writer.seen.is_none());
        assert!(!folder.join(summary::MARKDOWN_FILE).exists());
        let meeting = Meeting::load(&folder).expect("load");
        assert_eq!(meeting.status(Step::Summary), StepStatus::Failed);
    }

    #[test]
    fn a_summary_needs_a_transcript_first() {
        let folder = recorded_meeting("process-summary-early");
        let cancelled = Arc::new(AtomicBool::new(false));
        let result = summarize_now(&folder, &mut writer(), &cancelled, &mut |_| {});
        assert!(matches!(result, Err(ProcessError::NotTranscribed { .. })));
    }
}
