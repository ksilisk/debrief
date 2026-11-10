//! Turns recordings into transcripts and summaries, one meeting at a time, on a
//! worker thread. The steps are the same as `debrief process`; the window and
//! the menu bar follow the progress.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use debrief_core::meeting::{self, Meeting, Step, StepStatus};
use debrief_core::process::{self, Event, ProcessError, Transcriber};
use debrief_core::recording::Track;
use debrief_core::repair::repair;
use debrief_core::summary::Summarizer;
use debrief_summarize::{RemoteModel, config};
use debrief_transcribe::Whisper;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::library::{self, SHOWN_STEPS, Status};
use crate::phase::{self, Phase};
use crate::{notify, recording, speech_models, tray};

const PROGRESS: &str = "processing-progress";
const FINISHED: &str = "processing-finished";
/// The meeting list changed: a meeting started, finished or moved in the queue.
pub const MEETINGS_CHANGED: &str = "meetings-changed";

/// The window gets at most this many progress updates per second.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Everything that is not done yet, after a repair if the recording was cut off.
    Process,
    /// Only the summary, written again.
    Summarize,
}

struct Job {
    folder: PathBuf,
    kind: Kind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub folder: String,
    pub title: String,
    pub steps: Vec<StepProgress>,
    /// The track being transcribed, and how much of it is done, from 0 to 1.
    pub track: Option<Track>,
    pub track_done: f32,
    pub percent: u8,
    pub seconds_left: Option<u64>,
    pub summary_characters: usize,
    pub transcribe_model: String,
    pub summary_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepProgress {
    pub step: Step,
    pub status: StepStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finished {
    pub folder: String,
    pub title: String,
    /// What went wrong; `None` when everything that was asked for is done.
    pub problem: Option<String>,
    pub cancelled: bool,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Job>,
    current: Option<(PathBuf, Progress)>,
    worker: bool,
    /// Meetings that couldn't be transcribed because the model wasn't
    /// downloaded; they are processed once a download finishes.
    need_model: Vec<PathBuf>,
}

/// Kept in Tauri's managed state.
#[derive(Default)]
pub struct Queue {
    state: Mutex<State>,
    cancelled: Arc<AtomicBool>,
}

/// A meeting is being processed or waiting for it.
pub fn is_busy(app: &AppHandle) -> bool {
    lock(&app.state::<Queue>().state).worker
}

pub fn process(app: &AppHandle, folder: &Path) {
    enqueue(app, folder, Kind::Process);
}

/// A model finished downloading: processes the meetings that were waiting for one.
pub fn model_ready(app: &AppHandle) {
    let folders = std::mem::take(&mut lock(&app.state::<Queue>().state).need_model);
    for folder in folders {
        enqueue(app, &folder, Kind::Process);
    }
}

pub fn summarize(app: &AppHandle, folder: &Path) {
    enqueue(app, folder, Kind::Summarize);
}

/// The meeting went to the Trash, so it no longer waits for a model.
pub fn forget(app: &AppHandle, folder: &Path) {
    let folder = canonical(folder);
    lock(&app.state::<Queue>().state)
        .need_model
        .retain(|waiting| *waiting != folder);
}

/// Stops the meeting's processing, or takes it out of the queue. The steps that
/// are done stay done, so processing it again continues from there.
pub fn stop(app: &AppHandle, folder: &Path) {
    let queue = app.state::<Queue>();
    let folder = canonical(folder);
    let mut state = lock(&queue.state);
    state.waiting.retain(|job| job.folder != folder);
    if state
        .current
        .as_ref()
        .is_some_and(|(current, _)| *current == folder)
    {
        queue.cancelled.store(true, Ordering::SeqCst);
    }
    drop(state);
    emit(app, MEETINGS_CHANGED, ());
}

/// `Processing` or `Waiting` for a meeting in the queue.
pub fn status_of(app: &AppHandle, folder: &Path) -> Option<Status> {
    let folder = canonical(folder);
    let queue = app.state::<Queue>();
    let state = lock(&queue.state);
    if state
        .current
        .as_ref()
        .is_some_and(|(current, _)| *current == folder)
    {
        Some(Status::Processing)
    } else if state.waiting.iter().any(|job| job.folder == folder) {
        Some(Status::Waiting)
    } else {
        None
    }
}

pub fn current(app: &AppHandle) -> Option<Progress> {
    lock(&app.state::<Queue>().state)
        .current
        .as_ref()
        .map(|(_, progress)| progress.clone())
}

fn enqueue(app: &AppHandle, folder: &Path, kind: Kind) {
    let queue = app.state::<Queue>();
    let folder = canonical(folder);
    let mut state = lock(&queue.state);
    let queued = state.waiting.iter().any(|job| job.folder == folder)
        || state
            .current
            .as_ref()
            .is_some_and(|(current, _)| *current == folder);
    if !queued {
        state.waiting.push_back(Job { folder, kind });
    }
    let start_worker = !state.worker;
    state.worker = true;
    drop(state);
    emit(app, MEETINGS_CHANGED, ());
    if start_worker {
        let app = app.clone();
        thread::spawn(move || work(&app));
    }
}

/// Runs the jobs until the queue is empty. Whisper is loaded once per run.
fn work(app: &AppHandle) {
    let queue = app.state::<Queue>();
    let mut whisper: Option<Whisper> = None;
    loop {
        let job = {
            let mut state = lock(&queue.state);
            match state.waiting.pop_front() {
                Some(job) => job,
                None => {
                    state.worker = false;
                    state.current = None;
                    break;
                }
            }
        };
        queue.cancelled.store(false, Ordering::SeqCst);
        let finished = run(app, &job, &mut whisper, &queue.cancelled);
        lock(&queue.state).current = None;
        if finished.problem.is_none() && !finished.cancelled {
            notify::meeting_ready(app, &finished.title);
        }
        emit(app, FINISHED, finished);
        emit(app, MEETINGS_CHANGED, ());
        tray::refresh_recent(app);
    }
    if !recording::is_active(app) {
        phase::set(app, Phase::Idle);
    }
}

fn run(
    app: &AppHandle,
    job: &Job,
    whisper: &mut Option<Whisper>,
    cancelled: &Arc<AtomicBool>,
) -> Finished {
    let folder = &job.folder;
    let title = Meeting::load(folder)
        .map(|meeting| library::title(folder, &meeting))
        .unwrap_or_else(|_| folder.display().to_string());
    let finished = |problem: Option<String>, cancelled: bool| Finished {
        folder: folder.display().to_string(),
        title: title.clone(),
        problem,
        cancelled,
    };

    let (mut summarizer, setup_problem) = match RemoteModel::from_settings() {
        Ok(model) => (model, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let mut tracker = Tracker::new(
        app,
        folder,
        &title,
        summarizer.as_ref().map(|model| model.model().to_string()),
    );
    tracker.send(true);
    emit(app, MEETINGS_CHANGED, ());

    let result = match job.kind {
        Kind::Process => {
            if let Err(problem) = repair_if_needed(folder) {
                return finished(Some(problem), false);
            }
            let transcriber = match loaded(whisper) {
                Ok(transcriber) => transcriber,
                Err(Missing::Model(problem)) => {
                    let queue = app.state::<Queue>();
                    let mut state = lock(&queue.state);
                    if !state.need_model.contains(folder) {
                        state.need_model.push(folder.clone());
                    }
                    return finished(Some(problem), false);
                }
                Err(Missing::Other(problem)) => return finished(Some(problem), false),
            };
            transcriber.set_hints(&hints());
            tracker.progress.transcribe_model = transcriber.model().to_string();
            process::process(
                folder,
                transcriber,
                summarizer
                    .as_mut()
                    .map(|model| model as &mut dyn Summarizer),
                cancelled,
                &mut |event| tracker.event(event),
            )
        }
        Kind::Summarize => match summarizer.as_mut() {
            Some(model) => {
                process::summarize_now(folder, model, cancelled, &mut |event| tracker.event(event))
            }
            None => Err(ProcessError::NoSummaryProvider),
        },
    };
    match result {
        Ok(_) => finished(None, false),
        Err(ProcessError::Cancelled) => finished(None, true),
        Err(ProcessError::NoSummaryProvider) => {
            let reason = setup_problem.unwrap_or_else(|| "summaries are not set up".into());
            finished(
                Some(format!(
                    "The transcript is saved, but no summary was written: {reason}."
                )),
                false,
            )
        }
        Err(error) => finished(Some(capitalized(&error.to_string())), false),
    }
}

enum Missing {
    /// The chosen model isn't downloaded.
    Model(String),
    Other(String),
}

/// The chosen Whisper model, loaded the first time a job needs it and again
/// when another one is chosen.
fn loaded(whisper: &mut Option<Whisper>) -> Result<&mut Whisper, Missing> {
    let model = speech_models::chosen();
    if whisper
        .as_ref()
        .is_some_and(|loaded| loaded.model() != model.name)
    {
        *whisper = None;
    }
    if whisper.is_none() {
        let downloaded = model
            .is_downloaded()
            .map_err(|error| Missing::Other(error.to_string()))?;
        if !downloaded {
            return Err(Missing::Model(format!(
                "The speech model {} isn't downloaded. Download it on the New recording screen or in Settings; the meeting is processed when it finishes.",
                model.name
            )));
        }
        let path = model
            .path()
            .map_err(|error| Missing::Other(error.to_string()))?;
        *whisper = Some(
            Whisper::load(&path, model.name).map_err(|error| Missing::Other(error.to_string()))?,
        );
    }
    whisper
        .as_mut()
        .ok_or_else(|| Missing::Other("Whisper did not load".into()))
}

/// Names and terms from Settings, read again for each meeting.
fn hints() -> String {
    config::folder()
        .and_then(|folder| config::Settings::load(&folder))
        .map(|settings| settings.transcription.hints)
        .unwrap_or_default()
}

/// Like `debrief repair`: fixes a recording that was cut off before processing it.
fn repair_if_needed(folder: &Path) -> Result<(), String> {
    let Some(mut repaired) = repair(folder).map_err(|error| error.to_string())? else {
        return Ok(());
    };
    // A failed join keeps the parts; the transcript doesn't need the video.
    let _ = meeting::finish_video(folder, &mut repaired.meeting, debrief_capture::join_videos);
    repaired
        .meeting
        .save(folder)
        .map_err(|error| error.to_string())
}

/// Follows the events of one job for the window and the menu bar.
struct Tracker<'a> {
    app: &'a AppHandle,
    progress: Progress,
    started: Instant,
    /// Where transcribing stood at the first progress event, for the time left.
    first_fraction: Option<f32>,
    last_sent: Instant,
}

impl<'a> Tracker<'a> {
    fn new(app: &'a AppHandle, folder: &Path, title: &str, summary_model: Option<String>) -> Self {
        let meeting = Meeting::load(folder).ok();
        let steps = SHOWN_STEPS
            .iter()
            .map(|&step| StepProgress {
                step,
                status: match &meeting {
                    Some(meeting) if step == Step::Summary && !meeting.settings.summary => {
                        StepStatus::Skipped
                    }
                    Some(meeting) => match meeting.status(step) {
                        StepStatus::Running => StepStatus::Pending,
                        status => status,
                    },
                    None => StepStatus::Pending,
                },
            })
            .collect();
        Self {
            app,
            progress: Progress {
                folder: folder.display().to_string(),
                title: title.to_string(),
                steps,
                track: None,
                track_done: 0.0,
                percent: 0,
                seconds_left: None,
                summary_characters: 0,
                transcribe_model: speech_models::chosen().name.to_string(),
                summary_model,
            },
            started: Instant::now(),
            first_fraction: None,
            last_sent: Instant::now(),
        }
    }

    fn event(&mut self, event: Event) {
        let mut urgent = false;
        match event {
            Event::Started(step) => {
                self.set_status(step, StepStatus::Running);
                urgent = true;
            }
            Event::Finished(step) => {
                self.set_status(step, StepStatus::Done);
                urgent = true;
            }
            Event::Transcribing { track, done } => {
                self.progress.track = Some(track);
                self.progress.track_done = done;
                let fraction = (track_index(track) + done) / 2.0;
                let first = *self.first_fraction.get_or_insert(fraction);
                let elapsed = self.started.elapsed().as_secs_f32();
                self.progress.seconds_left = (fraction - first > 0.02)
                    .then(|| (elapsed * (1.0 - fraction) / (fraction - first)) as u64);
            }
            Event::Summarizing { characters } => self.progress.summary_characters = characters,
        }
        self.progress.percent = self.percent();
        self.send(urgent);
    }

    fn set_status(&mut self, step: Step, status: StepStatus) {
        if let Some(entry) = self
            .progress
            .steps
            .iter_mut()
            .find(|entry| entry.step == step)
        {
            entry.status = status;
        }
    }

    fn percent(&self) -> u8 {
        let status = |step| {
            self.progress
                .steps
                .iter()
                .find(|entry| entry.step == step)
                .map_or(StepStatus::Pending, |entry| entry.status)
        };
        let finished = |step| matches!(status(step), StepStatus::Done | StepStatus::Skipped);
        let share = if !finished(Step::Transcribe) {
            match self.progress.track {
                Some(track) => (track_index(track) + self.progress.track_done) / 2.0 * 0.9,
                None => 0.0,
            }
        } else if !finished(Step::Export) {
            0.92
        } else if !finished(Step::Summary) {
            0.95
        } else {
            1.0
        };
        (share * 100.0).round().clamp(0.0, 100.0) as u8
    }

    /// Sends the progress now when `urgent`, otherwise at most every [`PROGRESS_INTERVAL`].
    fn send(&mut self, urgent: bool) {
        if !urgent && self.last_sent.elapsed() < PROGRESS_INTERVAL {
            return;
        }
        self.last_sent = Instant::now();
        let folder = canonical(Path::new(&self.progress.folder));
        lock(&self.app.state::<Queue>().state).current = Some((folder, self.progress.clone()));
        emit(self.app, PROGRESS, self.progress.clone());
        if !recording::is_active(self.app) {
            phase::set(
                self.app,
                Phase::Processing {
                    title: self.progress.title.clone(),
                    step: step_label(&self.progress).into(),
                    percent: self.progress.percent,
                },
            );
        }
    }
}

fn track_index(track: Track) -> f32 {
    match track {
        Track::Me => 0.0,
        Track::Them => 1.0,
    }
}

/// For the menu bar: what is happening now.
fn step_label(progress: &Progress) -> &'static str {
    let running = progress
        .steps
        .iter()
        .find(|entry| entry.status == StepStatus::Running)
        .map(|entry| entry.step);
    match running {
        Some(Step::Merge) => "Merging",
        Some(Step::Export) => "Saving files",
        Some(Step::Summary) => "Writing the summary",
        _ => "Transcribing",
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn canonical(folder: &Path) -> PathBuf {
    folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf())
}

fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(error) = app.emit(event, payload) {
        eprintln!("Could not send {event} to the window: {error}");
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
