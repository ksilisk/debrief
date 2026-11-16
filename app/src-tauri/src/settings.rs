//! The Settings screen: what it shows and the changes it saves. Everything
//! here blocks, so the commands run it off the main thread.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use debrief_core::cleanup::{self, Delete, Usage};
use debrief_summarize::config::{self, ConfigError, Keys, Provider, Settings, masked};
use debrief_summarize::{HttpError, chat_models, list_models};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
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
    pub summary: SummaryView,
    pub cleanup: CleanupView,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupView {
    /// `None` never deletes anything.
    pub after_days: Option<u32>,
    pub delete: Delete,
    /// What the meetings take now.
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryView {
    /// `None` until summaries are set up.
    pub provider: Option<Provider>,
    /// With the `server` provider.
    pub server: Option<u32>,
    pub model: String,
    pub context_tokens: usize,
    pub keys: SavedKeys,
    pub servers: Vec<ServerView>,
}

/// The saved keys, masked.
#[derive(Debug, Clone, Serialize)]
pub struct SavedKeys {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
    pub openrouter: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerView {
    pub id: u32,
    pub label: String,
    pub url: String,
    /// The label, or the host.
    pub name: String,
    /// Masked; `None` when the server has no key.
    pub key: Option<String>,
}

/// What the screen changed; `None` keeps the saved value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub video: Option<bool>,
    pub fps: Option<u32>,
    pub context_tokens: Option<usize>,
}

/// What Test connection checks.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub provider: Provider,
    /// With the `server` provider, as typed on the screen.
    pub server: Option<ServerDraft>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDraft {
    /// `None` for a new server.
    pub id: Option<u32>,
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    pub models: Vec<ModelView>,
    /// The server's id; a new server has one now.
    pub server: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    /// Listed by Claude, OpenRouter and some servers; not by OpenAI.
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
    /// With the `server` provider.
    pub server: Option<u32>,
    pub model: String,
    pub context_tokens: usize,
    /// `None` uses the default limit.
    pub answer_tokens: Option<usize>,
}

pub fn view() -> Result<View, String> {
    let folder = config::folder().map_err(failed)?;
    let settings = Settings::load(&folder).map_err(failed)?;
    let keys = Keys::load(&folder).map_err(failed)?;
    let library = settings.library().map_err(failed)?;
    let usage = cleanup::usage(&library);
    let summary = settings.summary;
    Ok(View {
        meetings_folder: with_tilde(&library),
        video: settings.recording.video,
        fps: settings.recording.fps,
        summary: SummaryView {
            provider: summary.provider,
            server: summary.server,
            model: summary.model,
            context_tokens: summary.context_tokens,
            keys: SavedKeys {
                openai: keys.get(Provider::Openai.name()).map(masked),
                anthropic: keys.get(Provider::Anthropic.name()).map(masked),
                openrouter: keys.get(Provider::Openrouter.name()).map(masked),
            },
            servers: summary
                .servers
                .iter()
                .map(|server| ServerView {
                    id: server.id,
                    label: server.label.clone(),
                    url: server.url.clone(),
                    name: server.name(),
                    key: keys.get(&server.key_name()).map(masked),
                })
                .collect(),
        },
        cleanup: CleanupView {
            after_days: settings.cleanup.after_days,
            delete: settings.cleanup.delete,
            usage,
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
        if let Some(context) = changes.context_tokens.filter(|&tokens| tokens > 0) {
            settings.summary.context_tokens = context;
        }
    })
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

/// Checks the key by listing the models. A typed key that works is saved, and
/// so is a server that answers.
pub fn summary_models(target: Target, key: Option<String>) -> Result<ModelList, String> {
    let folder = config::folder().map_err(failed)?;
    let mut settings = Settings::load(&folder).map_err(failed)?;
    let mut keys = Keys::load(&folder).map_err(failed)?;
    let typed = key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    let provider = target.provider;

    let Some(draft) = target.server.filter(|_| provider == Provider::Server) else {
        let url = provider.url().ok_or("Choose a server.")?;
        let key = match (&typed, keys.get(provider.name())) {
            (Some(typed), _) => typed.clone(),
            (None, Some(saved)) => saved.to_string(),
            (None, None) => return Err("Paste an API key first.".into()),
        };
        let models = list_models(provider, url, Some(&key))
            .map_err(|error| connection_error(provider, url, error))?;
        if let Some(typed) = typed {
            keys.set(provider.name(), typed);
            keys.save(&folder).map_err(failed)?;
        }
        return Ok(ModelList {
            models: model_views(provider, models),
            server: None,
        });
    };

    let url = config::server_url(&draft.url)
        .map_err(|error| format!("{}.", capitalized(&error.to_string())))?;
    let saved = draft
        .id
        .and_then(|id| settings.summary.find_server(id))
        .cloned();
    // A saved key only goes to the address it was typed for.
    let same_url = saved.as_ref().is_some_and(|server| server.url == url);
    let key = typed.clone().or_else(|| {
        saved
            .as_ref()
            .filter(|_| same_url)
            .and_then(|server| keys.get(&server.key_name()))
            .map(str::to_string)
    });
    let models = list_models(provider, &url, key.as_deref())
        .map_err(|error| connection_error(provider, &url, error))?;
    let id = settings.summary.put_server(draft.id, &draft.label, &url);
    let key_name = config::server_key_name(id);
    match typed {
        Some(typed) => keys.set(&key_name, typed),
        None if !same_url => keys.remove(&key_name),
        None => {}
    }
    settings.save(&folder).map_err(failed)?;
    keys.save(&folder).map_err(failed)?;
    Ok(ModelList {
        models: model_views(provider, models),
        server: Some(id),
    })
}

fn model_views(provider: Provider, models: Vec<debrief_summarize::ModelInfo>) -> Vec<ModelView> {
    chat_models(provider, models)
        .into_iter()
        .map(|model| ModelView {
            answer_tokens: model.answer_limit(),
            context_tokens: model.context_tokens,
            suggested: Some(model.id.as_str()) == provider.suggested_model(),
            id: model.id,
        })
        .collect()
}

fn connection_error(provider: Provider, url: &str, error: HttpError) -> String {
    let host = config::host(url).unwrap_or_else(|| url.to_string());
    match error {
        HttpError::Key { .. } if provider == Provider::Server => {
            format!("The server wants an API key, or a different one: {error}.")
        }
        HttpError::Key { .. } => format!("The key didn't work: {error}."),
        HttpError::Network(_) if provider == Provider::Server => {
            format!("Nothing answers at {host}. Is the server running? ({error})")
        }
        HttpError::Status { status: 404, .. } if !url.ends_with("/v1") => {
            format!(
                "{}. Most servers' addresses end in /v1.",
                capitalized(&error.to_string())
            )
        }
        error => format!("{}.", capitalized(&error.to_string())),
    }
}

/// Saves the provider and model; a provider's key must be saved already.
pub fn use_summary_model(choice: ModelChoice) -> Result<(), String> {
    let folder = config::folder().map_err(failed)?;
    let settings = Settings::load(&folder).map_err(failed)?;
    let server = match choice.provider {
        Provider::Server => {
            let id = choice.server.ok_or("Choose a server.")?;
            settings
                .summary
                .find_server(id)
                .ok_or("That server is no longer in Settings.")?;
            Some(id)
        }
        provider => {
            if Keys::load(&folder)
                .map_err(failed)?
                .get(provider.name())
                .is_none()
            {
                return Err("Test the connection first, so the API key is saved.".into());
            }
            None
        }
    };
    let model = choice.model.trim().to_string();
    if model.is_empty() {
        return Err("Choose a model.".into());
    }
    update(|settings| {
        let summary = &mut settings.summary;
        summary.provider = Some(choice.provider);
        summary.server = server;
        summary.model = model;
        summary.context_tokens = choice.context_tokens.max(1);
        summary.answer_tokens = choice
            .answer_tokens
            .unwrap_or(config::DEFAULT_ANSWER_TOKENS)
            .max(1);
    })
}

pub fn rename_server(id: u32, label: String) -> Result<(), String> {
    update(|settings| {
        if let Some(server) = settings.summary.servers.iter_mut().find(|s| s.id == id) {
            server.label = label.trim().to_string();
        }
    })
}

/// Removes the server and its key.
pub fn remove_server(id: u32) -> Result<(), String> {
    let folder = config::folder().map_err(failed)?;
    let mut settings = Settings::load(&folder).map_err(failed)?;
    let Some(server) = settings.summary.remove_server(id) else {
        return Ok(());
    };
    settings.save(&folder).map_err(failed)?;
    let mut keys = Keys::load(&folder).map_err(failed)?;
    keys.remove(&server.key_name());
    keys.save(&folder).map_err(failed)
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

pub(crate) fn failed(error: ConfigError) -> String {
    capitalized(&error.to_string())
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
