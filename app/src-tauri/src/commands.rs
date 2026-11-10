//! The commands the Svelte window calls. Anything that touches the meetings
//! folder runs off the main thread: the first access can wait on macOS's
//! Documents permission prompt.

use std::io::Write;
use std::process::{Command, Stdio};

use tauri::AppHandle;

use crate::library::{self, Details, Item};
use crate::permissions::{self, Folder, Kind, Status};
use crate::processing::{self, Progress};
use crate::recording::{self, Choices, Defaults, Live, SourceList};
use crate::relaunch;
use crate::settings::{self, Changes, ModelChoice, ModelView, View};
use crate::speech_models;
use crate::storage;
use crate::updates;

#[tauri::command]
pub fn recording_defaults() -> Defaults {
    recording::defaults()
}

#[tauri::command]
pub async fn recording_sources() -> Result<SourceList, String> {
    blocking(recording::sources).await
}

#[tauri::command]
pub fn current_recording(app: AppHandle) -> Option<Live> {
    recording::live(&app)
}

#[tauri::command]
pub async fn start_recording(app: AppHandle, choices: Choices) -> Result<(), String> {
    blocking(move || recording::start(&app, choices)).await
}

#[tauri::command]
pub async fn toggle_pause(app: AppHandle) -> Result<(), String> {
    blocking(move || recording::toggle_pause(&app)).await
}

#[tauri::command]
pub async fn mark_moment(app: AppHandle) -> Result<(), String> {
    blocking(move || recording::mark(&app)).await
}

#[tauri::command]
pub async fn stop_recording(app: AppHandle) -> Result<(), String> {
    blocking(move || {
        recording::stop(&app, true);
        Ok(())
    })
    .await
}

/// Newest first.
#[tauri::command]
pub async fn meetings(app: AppHandle) -> Result<Vec<Item>, String> {
    blocking(move || Ok(library::list(&|folder| processing::status_of(&app, folder)))).await
}

#[tauri::command]
pub async fn meeting(app: AppHandle, folder: String) -> Result<Details, String> {
    blocking(move || {
        let folder = library::checked_folder(&folder)?;
        library::details(&folder, &|folder| processing::status_of(&app, folder))
    })
    .await
}

#[tauri::command]
pub async fn process_meeting(app: AppHandle, folder: String) -> Result<(), String> {
    blocking(move || {
        processing::process(&app, &library::checked_folder(&folder)?);
        Ok(())
    })
    .await
}

/// `prompt` becomes the meeting's own; `None` makes it use the default again.
#[tauri::command]
pub async fn write_summary(
    app: AppHandle,
    folder: String,
    prompt: Option<String>,
) -> Result<(), String> {
    blocking(move || {
        let folder = library::checked_folder(&folder)?;
        debrief_core::summary::set_own_prompt(&folder, prompt.as_deref())
            .map_err(|error| error.to_string())?;
        processing::summarize(&app, &folder);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn stop_processing(app: AppHandle, folder: String) -> Result<(), String> {
    blocking(move || {
        processing::stop(&app, &library::checked_folder(&folder)?);
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn processing_progress(app: AppHandle) -> Option<Progress> {
    processing::current(&app)
}

/// Shows a meeting folder in Finder.
#[tauri::command]
pub async fn show_in_finder(folder: String) -> Result<(), String> {
    blocking(move || {
        let folder = library::checked_folder(&folder)?;
        Command::new("open")
            .arg("-R")
            .arg(&folder)
            .status()
            .map_err(|error| format!("Could not open Finder: {error}"))?;
        Ok(())
    })
    .await
}

/// Asks first; `false` when nothing was moved. Problems show in an alert.
#[tauri::command]
pub async fn delete_meeting(app: AppHandle, folder: String) -> Result<bool, String> {
    blocking(move || {
        Ok(storage::delete_meeting(
            &app,
            &library::checked_folder(&folder)?,
        ))
    })
    .await
}

/// Puts text on the clipboard through `pbcopy`, which the web view can't always reach.
#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("Could not copy: {error}");
    let mut pbcopy = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(failed)?;
    if let Some(mut stdin) = pbcopy.stdin.take() {
        stdin.write_all(text.as_bytes()).map_err(failed)?;
    }
    pbcopy.wait().map_err(failed)?;
    Ok(())
}

#[tauri::command]
pub async fn settings() -> Result<View, String> {
    blocking(settings::view).await
}

#[tauri::command]
pub async fn save_settings(changes: Changes) -> Result<(), String> {
    blocking(move || settings::save(changes)).await
}

/// Asks first when the new setting deletes something now; `false` when the
/// answer was no and the setting stays as it was.
#[tauri::command]
pub async fn set_cleanup(
    app: AppHandle,
    after_days: Option<u32>,
    delete: debrief_core::cleanup::Delete,
) -> Result<bool, String> {
    blocking(move || {
        storage::change(
            &app,
            debrief_summarize::config::CleanupSettings { after_days, delete },
        )
    })
    .await
}

/// The new folder, or `None` when the picker was cancelled.
#[tauri::command]
pub async fn choose_meetings_folder(app: AppHandle) -> Result<Option<String>, String> {
    blocking(move || settings::choose_meetings_folder(&app)).await
}

#[tauri::command]
pub async fn summary_models(
    provider: debrief_summarize::config::Provider,
    key: Option<String>,
) -> Result<Vec<ModelView>, String> {
    blocking(move || settings::summary_models(provider, key)).await
}

#[tauri::command]
pub async fn use_summary_model(choice: ModelChoice) -> Result<(), String> {
    blocking(move || settings::use_summary_model(choice)).await
}

#[tauri::command]
pub async fn edit_summary_prompt() -> Result<(), String> {
    blocking(settings::edit_summary_prompt).await
}

#[tauri::command]
pub async fn permissions() -> Result<permissions::View, String> {
    blocking(|| Ok(permissions::view())).await
}

/// Waits for the user to answer the system prompt.
#[tauri::command]
pub async fn request_permission(kind: Kind) -> Result<Status, String> {
    blocking(move || Ok(permissions::request(kind))).await
}

/// Can wait for the answer to macOS's folder prompt.
#[tauri::command]
pub async fn meetings_folder_access() -> Result<Folder, String> {
    blocking(|| Ok(permissions::folder())).await
}

#[tauri::command]
pub async fn open_privacy_settings(kind: Kind) -> Result<(), String> {
    blocking(move || permissions::open_settings(kind)).await
}

#[tauri::command]
pub async fn relaunch(app: AppHandle) -> Result<(), String> {
    blocking(move || relaunch::relaunch(&app)).await
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> updates::Status {
    updates::status(&app)
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> updates::Status {
    updates::check(&app).await;
    updates::status(&app)
}

/// Quits and opens the new version when it works.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    updates::install(&app).await
}

#[tauri::command]
pub async fn speech_models(app: AppHandle) -> Result<speech_models::View, String> {
    blocking(move || Ok(speech_models::view(&app))).await
}

/// Starts the download; `speech-models` events follow it.
#[tauri::command]
pub fn download_model(app: AppHandle, name: String) -> Result<(), String> {
    speech_models::download(&app, &name)
}

#[tauri::command]
pub fn cancel_model_download(app: AppHandle) {
    speech_models::cancel(&app);
}

#[tauri::command]
pub async fn choose_model(app: AppHandle, name: String) -> Result<(), String> {
    blocking(move || speech_models::choose(&app, &name)).await
}

#[tauri::command]
pub async fn delete_model(app: AppHandle, name: String) -> Result<(), String> {
    blocking(move || speech_models::delete(&app, &name)).await
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}
