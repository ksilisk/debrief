//! The recording in progress: starting it from the window, the menu or the
//! shortcut, its live status for the window and the menu bar, and stopping it.
//! Everything here blocks, so callers run it off the main thread.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use chrono::Local;
use debrief_capture::{
    Capture, CaptureError, MicrophoneSource, Options, SystemAudio, Video, VideoTarget,
};
use debrief_core::meeting::{self, Language, Meeting, Settings, VIDEO_FILE, VideoFile};
use debrief_core::recorder::{Recorder, RecorderError, Status};
use debrief_summarize::config::{self, Provider, RecordingSettings};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::phase::{self, Phase};
use crate::{hotkey, processing, tray};

/// How often the window gets the timer and the levels.
const TICK: Duration = Duration::from_millis(100);

const STARTED: &str = "recording-started";
const STATUS: &str = "recording-status";
const STOPPING: &str = "recording-stopping";
const STOPPED: &str = "recording-stopped";
/// A recording started from the menu or the shortcut could not start.
const FAILED: &str = "recording-failed";

/// What the New recording screen sends.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choices {
    pub title: String,
    pub record_microphone: bool,
    pub microphone: Option<String>,
    pub record_system_audio: bool,
    pub app: Option<String>,
    pub video: bool,
    pub fps: u32,
    /// `None` records the main screen.
    pub video_source: Option<VideoSource>,
    pub language: Language,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum VideoSource {
    Screen(u32),
    Window(u32),
}

/// The last choices, and whether summaries can be written at all.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub record_microphone: bool,
    pub microphone: Option<String>,
    pub record_system_audio: bool,
    pub app: Option<String>,
    pub video: bool,
    pub fps: u32,
    pub video_source: Option<VideoSource>,
    pub language: Language,
    /// The model that writes summaries; `None` until summaries are set up.
    pub summary_model: Option<String>,
    pub summary_provider: Option<Provider>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceList {
    pub microphones: Vec<Choice<String>>,
    pub apps: Vec<Choice<String>>,
    pub screens: Vec<Choice<u32>>,
    pub windows: Vec<Choice<u32>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice<T> {
    pub id: T,
    pub label: String,
}

/// What the Recording screen shows besides the live status.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub folder: String,
    pub title: String,
    /// `None` when the microphone is switched off.
    pub microphone: Option<String>,
    /// `None` when the system audio is switched off.
    pub system_audio: Option<String>,
    /// The recorded screen or window with its size and frame rate; `None` for audio only.
    pub video: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub seconds: f64,
    pub paused: bool,
    pub marks: usize,
    /// The loudest sample since the last status, from 0 to 1.
    pub me_level: f32,
    pub them_level: f32,
    /// Audio and video files together.
    pub bytes: u64,
}

impl From<&Status> for LiveStatus {
    fn from(status: &Status) -> Self {
        Self {
            seconds: status.position,
            paused: status.paused,
            marks: status.marks,
            me_level: status.me_peak,
            them_level: status.them_peak,
            bytes: [&status.me, &status.them]
                .into_iter()
                .flatten()
                .map(|stats| stats.bytes())
                .sum::<u64>()
                + status.video_bytes.unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub title: String,
    pub folder: String,
    /// Things that went wrong; the files that could be saved are kept.
    pub problems: Vec<String>,
}

struct Active {
    id: u64,
    recorder: Recorder<Capture>,
    live: Live,
}

/// Kept in Tauri's managed state.
#[derive(Default)]
pub struct Current {
    active: Mutex<Option<Active>>,
    /// Set while a recording starts or stops, so a second press does nothing.
    busy: AtomicBool,
    next_id: AtomicU64,
}

pub fn is_active(app: &AppHandle) -> bool {
    lock(&app.state::<Current>().active).is_some()
}

pub fn live(app: &AppHandle) -> Option<Live> {
    lock(&app.state::<Current>().active)
        .as_ref()
        .map(|active| active.live.clone())
}

pub fn defaults() -> Defaults {
    let settings = config::folder()
        .and_then(|folder| config::Settings::load(&folder))
        .unwrap_or_default();
    let recording = settings.recording;
    let summary_model = settings
        .summary
        .provider
        .map(|_| settings.summary.model.clone());
    Defaults {
        record_microphone: recording.record_microphone,
        microphone: recording.microphone,
        record_system_audio: recording.record_system_audio,
        app: recording.app,
        video: recording.video,
        fps: recording.fps,
        video_source: recording.screen.map(VideoSource::Screen),
        language: recording.language,
        summary_model,
        summary_provider: settings.summary.provider,
    }
}

/// What the next Start from the menu or the shortcut leaves out, if anything.
pub fn switched_off() -> Option<&'static str> {
    let defaults = defaults();
    match (defaults.record_microphone, defaults.record_system_audio) {
        (true, true) => None,
        (false, true) => Some("microphone off"),
        (true, false) => Some("system audio off"),
        (false, false) => Some("microphone and system audio off"),
    }
}

pub fn sources() -> Result<SourceList, String> {
    let sources = debrief_capture::sources().map_err(|error| explain(&error))?;
    let mut apps: Vec<Choice<String>> = sources
        .apps
        .into_iter()
        .map(|app| Choice {
            id: app.bundle_id,
            label: app.name,
        })
        .collect();
    apps.sort_by_key(|app| app.label.to_lowercase());
    Ok(SourceList {
        microphones: sources
            .microphones
            .into_iter()
            .map(|mic| Choice {
                label: if mic.is_default {
                    format!("{} (default)", mic.name)
                } else {
                    mic.name
                },
                id: mic.id,
            })
            .collect(),
        apps,
        screens: sources
            .displays
            .iter()
            .enumerate()
            .map(|(n, display)| Choice {
                id: display.id,
                label: format!("Screen {} · {} × {}", n + 1, display.width, display.height),
            })
            .collect(),
        windows: sources
            .windows
            .into_iter()
            .map(|window| Choice {
                id: window.id,
                label: if window.title.is_empty() {
                    window.app
                } else {
                    format!("{} — {}", window.app, window.title)
                },
            })
            .collect(),
    })
}

/// Starts with the last choices, for the menu and the shortcut.
pub fn start_with_defaults(app: &AppHandle) {
    let defaults = defaults();
    let choices = Choices {
        title: String::new(),
        record_microphone: defaults.record_microphone,
        microphone: defaults.microphone,
        record_system_audio: defaults.record_system_audio,
        app: defaults.app,
        video: defaults.video,
        fps: defaults.fps,
        video_source: defaults.video_source,
        language: defaults.language,
    };
    if let Err(message) = start(app, choices) {
        crate::window::show(app, Some(crate::window::Screen::New));
        if let Err(error) = app.emit(FAILED, message) {
            eprintln!("Could not tell the window: {error}");
        }
    }
}

pub fn start(app: &AppHandle, choices: Choices) -> Result<(), String> {
    let state = app.state::<Current>();
    if state.busy.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let started = if is_active(app) {
        Ok(())
    } else {
        start_now(app, &state, choices)
    };
    state.busy.store(false, Ordering::SeqCst);
    started
}

fn start_now(app: &AppHandle, state: &Current, choices: Choices) -> Result<(), String> {
    if !choices.record_microphone && !choices.record_system_audio {
        return Err(
            "Turn on the microphone or the system audio: there is nothing to transcribe.".into(),
        );
    }
    remember(&choices);
    let pressed_at = Local::now().fixed_offset();
    let library = config::library().map_err(|error| error.to_string())?;
    let folder =
        meeting::create_folder(&library, &meeting::folder_name(&pressed_at, &choices.title))
            .map_err(|error| error.to_string())?;

    let mut draft = Meeting::new(
        choices.title.trim().to_string(),
        pressed_at,
        Settings {
            language: choices.language,
        },
    );
    if !choices.record_microphone {
        draft.tracks.me = None;
    }
    if !choices.record_system_audio {
        draft.tracks.them = None;
    }
    draft.video = choices.video.then(|| VideoFile {
        file: VIDEO_FILE.to_string(),
        source: String::new(),
        fps: choices.fps,
        bytes: None,
        parts: Vec::new(),
    });
    let target = match choices.video_source {
        Some(VideoSource::Screen(id)) => VideoTarget::Display(id),
        Some(VideoSource::Window(id)) => VideoTarget::Window(id),
        None => VideoTarget::MainDisplay,
    };
    let recorder = Recorder::start(&folder, draft, |video, sink| {
        let options = Options {
            microphone: choices
                .record_microphone
                .then(|| match &choices.microphone {
                    Some(id) => MicrophoneSource::Device { id: id.clone() },
                    None => MicrophoneSource::Default,
                }),
            system_audio: choices.record_system_audio.then(|| match &choices.app {
                Some(bundle_id) => SystemAudio::App {
                    bundle_id: bundle_id.clone(),
                },
                None => SystemAudio::AllApps,
            }),
            video: video.map(|path| Video {
                target,
                fps: choices.fps,
                path,
            }),
        };
        Capture::start(&options, sink)
    })
    .map_err(|error| match &error {
        RecorderError::Capture(inner) => match inner.downcast_ref::<CaptureError>() {
            Some(capture_error) => explain(capture_error),
            None => error.to_string(),
        },
        _ => error.to_string(),
    })?;

    let sources = recorder.sources().clone();
    let title = match choices.title.trim() {
        "" => folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        title => title.to_string(),
    };
    let live = Live {
        folder: folder.display().to_string(),
        title: title.clone(),
        microphone: sources.microphone,
        system_audio: sources.system_audio,
        video: sources.video,
    };
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    *lock(&state.active) = Some(Active {
        id,
        recorder,
        live: live.clone(),
    });

    phase::set(
        app,
        Phase::Recording {
            title,
            elapsed_seconds: 0.0,
            paused: false,
        },
    );
    hotkey::register_mark(app);
    emit(app, STARTED, live);
    emit(app, processing::MEETINGS_CHANGED, ());
    let app = app.clone();
    thread::spawn(move || watch(&app, id));
    Ok(())
}

/// Sends the status to the window and the menu bar until the recording stops,
/// and stops it when the capture fails.
fn watch(app: &AppHandle, id: u64) {
    let mut shown = None;
    loop {
        thread::sleep(TICK);
        let (status, title) = match lock(&app.state::<Current>().active).as_ref() {
            Some(active) if active.id == id => {
                (active.recorder.status(), active.live.title.clone())
            }
            _ => return,
        };
        emit(app, STATUS, LiveStatus::from(&status));
        let menu_bar = (status.position as u64, status.paused);
        if shown != Some(menu_bar) {
            shown = Some(menu_bar);
            phase::set(
                app,
                Phase::Recording {
                    title,
                    elapsed_seconds: status.position,
                    paused: status.paused,
                },
            );
        }
        if status.error.is_some() {
            stop(app, true);
            return;
        }
    }
}

pub fn toggle_pause(app: &AppHandle) -> Result<(), String> {
    match lock(&app.state::<Current>().active).as_mut() {
        Some(active) => active
            .recorder
            .toggle_pause()
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

pub fn mark(app: &AppHandle) -> Result<(), String> {
    match lock(&app.state::<Current>().active).as_ref() {
        Some(active) => active
            .recorder
            .mark()
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

/// Whether quitting now would lose a recording.
pub fn needs_saving(app: &AppHandle) -> bool {
    let state = app.state::<Current>();
    state.busy.load(Ordering::SeqCst) || lock(&state.active).is_some()
}

/// Saves the recording before the app quits. Waits for a start or a stop that
/// is under way.
pub fn save_before_quit(app: &AppHandle) {
    while needs_saving(app) {
        stop(app, false);
        thread::sleep(Duration::from_millis(100));
    }
}

/// For when macOS ends the app without asking it first (Quit in the Dock,
/// logging out): saves on this thread, without the window or the menu bar.
pub fn save_at_exit(app: &AppHandle) {
    let active = lock(&app.state::<Current>().active).take();
    if let Some(active) = active {
        let saved = save(active);
        for problem in saved.problems {
            eprintln!("{problem}");
        }
    }
}

/// Stops and saves the recording, if there is one, and then processes it when
/// `then_process` is set.
pub fn stop(app: &AppHandle, then_process: bool) {
    let state = app.state::<Current>();
    if state.busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let active = lock(&state.active).take();
    if let Some(active) = active {
        hotkey::unregister_mark(app);
        emit(app, STOPPING, ());
        let saved = save(active);
        phase::set(app, Phase::Idle);
        if then_process {
            processing::process(app, Path::new(&saved.folder));
        }
        emit(app, STOPPED, saved);
        emit(app, processing::MEETINGS_CHANGED, ());
        tray::refresh_recent(app);
    }
    state.busy.store(false, Ordering::SeqCst);
}

fn save(active: Active) -> Saved {
    let folder = active.recorder.folder().to_path_buf();
    let mut problems = Vec::new();
    match active.recorder.stop() {
        Ok(stopped) => {
            if let Some(error) = stopped.meeting.recording.error {
                problems.push(format!("The recording stopped early: {error}"));
            }
            if let Some(error) = stopped.video_error {
                problems.push(format!("{error}. The video parts are kept in the folder."));
            }
            if let Some(error) = stopped.capture_error {
                problems.push(format!("The capture did not stop cleanly: {error}"));
            }
        }
        Err(error) => problems.push(format!("Could not finish saving: {error}")),
    }
    Saved {
        title: active.live.title,
        folder: folder.display().to_string(),
        problems,
    }
}

fn remember(choices: &Choices) {
    let Ok(folder) = config::folder() else {
        return;
    };
    // Never overwrite a settings file that could not be read.
    let Ok(mut settings) = config::Settings::load(&folder) else {
        return;
    };
    settings.recording = RecordingSettings {
        record_microphone: choices.record_microphone,
        microphone: choices.microphone.clone(),
        record_system_audio: choices.record_system_audio,
        app: choices.app.clone(),
        video: choices.video,
        fps: choices.fps,
        screen: match choices.video_source {
            Some(VideoSource::Screen(id)) => Some(id),
            _ => settings.recording.screen,
        },
        language: choices.language,
    };
    if let Err(error) = settings.save(&folder) {
        eprintln!("Could not save the recording choices: {error}");
    }
}

/// The capture's errors, in words for the app instead of the command line.
fn explain(error: &CaptureError) -> String {
    match error {
        CaptureError::Permission(_) => {
            "Debrief is not allowed to record the screen and system audio. \
             Turn on Debrief in System Settings > Privacy & Security > Screen & System Audio \
             Recording, then quit and reopen Debrief."
                .into()
        }
        CaptureError::NotFound(what) => {
            format!("{what} was not found. Pick it again from the list.")
        }
        CaptureError::Unsupported | CaptureError::Platform(_) => error.to_string(),
    }
}

fn emit<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(error) = app.emit(event, payload) {
        eprintln!("Could not send {event} to the window: {error}");
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
