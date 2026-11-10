//! The Whisper models on this Mac: the one that transcribes, downloading one
//! in the background with its progress, and deleting the others.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use debrief_summarize::config::{self, Settings};
use debrief_transcribe::models::{self, MODELS, Model, ModelError};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::processing;

const STATUS: &str = "speech-models";
/// The window gets at most this many progress updates per second.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// The model that transcribes.
    pub chosen: String,
    pub models: Vec<ModelView>,
    pub download: Option<DownloadView>,
    /// Why the last download failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub name: String,
    pub bytes: u64,
    pub downloaded: bool,
    /// What an unfinished download already has.
    pub partial_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadView {
    pub model: String,
    pub done: u64,
    pub total: u64,
    pub seconds_left: Option<u64>,
}

struct Active {
    model: Model,
    done: u64,
    started: Instant,
    done_at_start: u64,
}

/// Kept in Tauri's managed state.
#[derive(Default)]
pub struct Downloads {
    active: Mutex<Option<Active>>,
    error: Mutex<Option<String>>,
    cancelled: Arc<AtomicBool>,
}

/// The model the settings name, or the default.
pub fn chosen() -> Model {
    let settings = config::folder()
        .and_then(|folder| Settings::load(&folder))
        .unwrap_or_default();
    models::chosen(settings.transcription.model.as_deref())
}

pub fn view(app: &AppHandle) -> View {
    let downloads = app.state::<Downloads>();
    let download = lock(&downloads.active).as_ref().map(|active| {
        let seconds = active.started.elapsed().as_secs_f64();
        let fetched = active.done.saturating_sub(active.done_at_start) as f64;
        let speed = if seconds > 2.0 {
            fetched / seconds
        } else {
            0.0
        };
        DownloadView {
            model: active.model.name.to_string(),
            done: active.done,
            total: active.model.bytes,
            seconds_left: (speed > 0.0)
                .then(|| ((active.model.bytes - active.done) as f64 / speed) as u64),
        }
    });
    View {
        chosen: chosen().name.to_string(),
        models: MODELS
            .iter()
            .map(|model| ModelView {
                name: model.name.to_string(),
                bytes: model.bytes,
                downloaded: model.is_downloaded().unwrap_or(false),
                partial_bytes: model.partial_bytes(),
            })
            .collect(),
        download,
        error: lock(&downloads.error).clone(),
    }
}

/// Starts downloading in the background; the window follows it through events.
pub fn download(app: &AppHandle, name: &str) -> Result<(), String> {
    let model = models::by_name(name).ok_or_else(|| format!("There is no model {name}."))?;
    let downloads = app.state::<Downloads>();
    {
        let mut active = lock(&downloads.active);
        if let Some(current) = active.as_ref() {
            return Err(format!(
                "Debrief is already downloading {}.",
                current.model.name
            ));
        }
        let done = model.partial_bytes();
        *active = Some(Active {
            model,
            done,
            started: Instant::now(),
            done_at_start: done,
        });
    }
    *lock(&downloads.error) = None;
    downloads.cancelled.store(false, Ordering::SeqCst);
    emit(app);

    let app = app.clone();
    thread::spawn(move || {
        let downloads = app.state::<Downloads>();
        let mut sent = Instant::now();
        let result = model.download(
            &mut |done, _total| {
                if let Some(active) = lock(&downloads.active).as_mut() {
                    active.done = done;
                }
                if sent.elapsed() >= PROGRESS_INTERVAL {
                    sent = Instant::now();
                    emit(&app);
                }
            },
            &downloads.cancelled,
        );
        *lock(&downloads.active) = None;
        match result {
            Ok(_) => {
                if !chosen().is_downloaded().unwrap_or(false)
                    && let Err(error) = save_choice(model)
                {
                    eprintln!("Could not choose the downloaded model: {error}");
                }
                emit(&app);
                processing::model_ready(&app);
            }
            Err(ModelError::Cancelled) => emit(&app),
            Err(error) => {
                *lock(&downloads.error) = Some(capitalized(&error.to_string()));
                emit(&app);
            }
        }
    });
    Ok(())
}

/// Stops the download; the next one continues from where it stopped.
pub fn cancel(app: &AppHandle) {
    app.state::<Downloads>()
        .cancelled
        .store(true, Ordering::SeqCst);
}

/// The next meeting is transcribed with this model.
pub fn choose(app: &AppHandle, name: &str) -> Result<(), String> {
    let model = models::by_name(name).ok_or_else(|| format!("There is no model {name}."))?;
    if !model.is_downloaded().unwrap_or(false) {
        return Err("Download the model before you choose it.".into());
    }
    save_choice(model)?;
    emit(app);
    Ok(())
}

pub fn delete(app: &AppHandle, name: &str) -> Result<(), String> {
    let model = models::by_name(name).ok_or_else(|| format!("There is no model {name}."))?;
    if chosen() == model {
        return Err("Choose another model before you delete this one.".into());
    }
    if lock(&app.state::<Downloads>().active)
        .as_ref()
        .is_some_and(|active| active.model == model)
    {
        return Err("Stop the download before you delete the model.".into());
    }
    model
        .delete()
        .map_err(|error| capitalized(&error.to_string()))?;
    emit(app);
    Ok(())
}

fn save_choice(model: Model) -> Result<(), String> {
    let folder = config::folder().map_err(|error| error.to_string())?;
    let mut settings = Settings::load(&folder).map_err(|error| error.to_string())?;
    settings.transcription.model = (model != models::DEFAULT).then(|| model.name.to_string());
    settings.save(&folder).map_err(|error| error.to_string())
}

fn emit(app: &AppHandle) {
    if let Err(error) = app.emit(STATUS, view(app)) {
        eprintln!("Could not send the model status to the window: {error}");
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
