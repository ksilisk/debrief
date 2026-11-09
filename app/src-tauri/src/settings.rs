//! The Settings screen: what it shows and the changes it saves. Everything
//! here blocks, so the commands run it off the main thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use debrief_summarize::config::{self, ConfigError, Keys, Provider, Settings, masked};
use debrief_summarize::{chat_models, list_models};
use debrief_transcribe::hints::{self, MAX_TOKENS, Vocab};
use debrief_transcribe::models::LARGE_V3_TURBO;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

use crate::processing::{self, MEETINGS_CHANGED};
use crate::{recording, tray};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    /// With `~` for the home folder.
    pub meetings_folder: String,
    pub video: bool,
    pub fps: u32,
    pub hints: String,
    /// `None` until the Whisper model is downloaded.
    pub hint_tokens: Option<usize>,
    pub max_hint_tokens: usize,
    pub whisper: WhisperView,
    pub summary: SummaryView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperView {
    pub name: String,
    pub bytes: u64,
    pub downloaded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryView {
    /// `None` until summaries are set up.
    pub provider: Option<Provider>,
    pub model: String,
    pub context_tokens: usize,
    pub on_by_default: bool,
    pub keys: SavedKeys,
}

/// The saved keys, masked.
#[derive(Debug, Clone, Serialize)]
pub struct SavedKeys {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
}

/// What the screen changed; `None` keeps the saved value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub video: Option<bool>,
    pub fps: Option<u32>,
    pub hints: Option<String>,
    pub summaries_on: Option<bool>,
    pub context_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    /// Known for Claude models; OpenAI doesn't list it.
    pub context_tokens: Option<usize>,
    pub answer_tokens: usize,
    /// Chosen first when nothing is chosen yet.
    pub suggested: bool,
}

/// The model the screen picked, with the limits it knows for it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    pub provider: Provider,
    pub model: String,
    pub context_tokens: usize,
    /// `None` uses the default limit.
    pub answer_tokens: Option<usize>,
}

/// Whisper's vocabulary for counting hint tokens, read once. Kept in Tauri's managed state.
#[derive(Default)]
pub struct HintCounter(Mutex<Option<Arc<Vocab>>>);

pub fn view(app: &AppHandle) -> Result<View, String> {
    let folder = config::folder().map_err(failed)?;
    let settings = Settings::load(&folder).map_err(failed)?;
    let keys = Keys::load(&folder).map_err(failed)?;
    let library = settings.library().map_err(failed)?;
    let hint_tokens = count_hints(app, &settings.transcription.hints);
    let summary = settings.summary;
    Ok(View {
        meetings_folder: with_tilde(&library),
        video: settings.recording.video,
        fps: settings.recording.fps,
        hints: settings.transcription.hints,
        hint_tokens,
        max_hint_tokens: MAX_TOKENS,
        whisper: WhisperView {
            name: LARGE_V3_TURBO.name.to_string(),
            bytes: LARGE_V3_TURBO.bytes,
            downloaded: LARGE_V3_TURBO.is_downloaded().unwrap_or(false),
        },
        summary: SummaryView {
            provider: summary.provider,
            model: summary.model,
            context_tokens: summary.context_tokens,
            on_by_default: summary.on_by_default,
            keys: SavedKeys {
                openai: keys.get(Provider::Openai).map(masked),
                anthropic: keys.get(Provider::Anthropic).map(masked),
            },
        },
    })
}

pub fn save(changes: Changes) -> Result<(), String> {
    update(|settings| {
        if let Some(video) = changes.video {
            settings.recording.video = video;
        }
        if let Some(fps) = changes.fps {
            settings.recording.fps = fps;
        }
        if let Some(hints) = changes.hints {
            settings.transcription.hints = hints;
        }
        if let Some(on) = changes.summaries_on {
            settings.summary.on_by_default = on;
        }
        if let Some(context) = changes.context_tokens.filter(|&tokens| tokens > 0) {
            settings.summary.context_tokens = context;
        }
    })
}

/// Tokens in the prompt Whisper gets from these hints; `None` until the model is downloaded.
pub fn count_hints(app: &AppHandle, hints: &str) -> Option<usize> {
    let counter = app.state::<HintCounter>();
    let mut vocab = lock(&counter.0);
    if vocab.is_none() && LARGE_V3_TURBO.is_downloaded().unwrap_or(false) {
        let path = LARGE_V3_TURBO.path().ok()?;
        match Vocab::load(&path) {
            Ok(loaded) => *vocab = Some(Arc::new(loaded)),
            Err(error) => eprintln!("{error}"),
        }
    }
    let prompt = hints::prompt(hints);
    vocab.as_ref().map(|vocab| vocab.count(&prompt))
}

/// Asks for a folder and saves it. `None` when the picker was cancelled.
pub fn choose_meetings_folder(app: &AppHandle) -> Result<Option<String>, String> {
    if recording::is_active(app) {
        return Err("Stop the recording before you change the folder.".into());
    }
    if processing::is_busy(app) {
        return Err("Wait until processing finishes before you change the folder.".into());
    }
    let current = config::library().map_err(failed)?;
    let start = current
        .ancestors()
        .find(|folder| folder.is_dir())
        .map_or_else(|| current.clone(), Path::to_path_buf);
    let Some(picked) = app
        .dialog()
        .file()
        .set_title("Save meetings to")
        .set_directory(start)
        .set_can_create_directories(true)
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let picked = picked
        .into_path()
        .map_err(|error| format!("Could not use that folder: {error}"))?;
    if !fs::metadata(&picked).is_ok_and(|meta| meta.is_dir()) {
        return Err(format!("{} is not a folder.", picked.display()));
    }
    let default = debrief_core::meeting::default_library();
    update(|settings| {
        settings.meetings_folder = (Some(&picked) != default.as_ref()).then(|| picked.clone());
    })?;
    if let Err(error) = app.emit(MEETINGS_CHANGED, ()) {
        eprintln!("Could not refresh the meeting list: {error}");
    }
    tray::refresh_recent(app);
    Ok(Some(with_tilde(&picked)))
}

/// Checks the key by listing the models; a typed key that works is saved.
pub fn summary_models(provider: Provider, key: Option<String>) -> Result<Vec<ModelView>, String> {
    let folder = config::folder().map_err(failed)?;
    let mut keys = Keys::load(&folder).map_err(failed)?;
    let typed = key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    let key = match (&typed, keys.get(provider)) {
        (Some(typed), _) => typed.clone(),
        (None, Some(saved)) => saved.to_string(),
        (None, None) => return Err("Paste an API key first.".into()),
    };
    let models =
        list_models(provider, &key).map_err(|error| format!("The key didn't work: {error}."))?;
    if let Some(typed) = typed {
        keys.set(provider, typed);
        keys.save(&folder).map_err(failed)?;
    }
    Ok(chat_models(provider, models)
        .into_iter()
        .map(|model| ModelView {
            answer_tokens: model.answer_limit(),
            context_tokens: model.context_tokens,
            suggested: model.id == provider.suggested_model(),
            id: model.id,
        })
        .collect())
}

/// Saves the provider and model; their key must be saved already.
pub fn use_summary_model(choice: ModelChoice) -> Result<(), String> {
    let folder = config::folder().map_err(failed)?;
    if Keys::load(&folder)
        .map_err(failed)?
        .get(choice.provider)
        .is_none()
    {
        return Err("Test the connection first, so the API key is saved.".into());
    }
    let model = choice.model.trim().to_string();
    if model.is_empty() {
        return Err("Choose a model.".into());
    }
    update(|settings| {
        let summary = &mut settings.summary;
        summary.provider = Some(choice.provider);
        summary.model = model;
        summary.context_tokens = choice.context_tokens.max(1);
        summary.answer_tokens = choice
            .answer_tokens
            .unwrap_or(config::DEFAULT_ANSWER_TOKENS)
            .max(1);
    })
}

/// Opens the summary instructions in the default text editor.
pub fn edit_summary_prompt() -> Result<(), String> {
    let folder = config::folder().map_err(failed)?;
    config::prompt(&folder).map_err(failed)?;
    let opened = Command::new("open")
        .arg("-t")
        .arg(folder.join(config::PROMPT_FILE))
        .status()
        .map_err(|error| format!("Could not open the instructions: {error}"))?;
    if !opened.success() {
        return Err("Could not open the instructions in a text editor.".into());
    }
    Ok(())
}

fn update(change: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let folder = config::folder().map_err(failed)?;
    let mut settings = Settings::load(&folder).map_err(failed)?;
    change(&mut settings);
    settings.save(&folder).map_err(failed)
}

pub(crate) fn with_tilde(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn failed(error: ConfigError) -> String {
    let text = error.to_string();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
