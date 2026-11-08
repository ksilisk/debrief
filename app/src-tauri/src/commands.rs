//! The commands the Svelte window calls. Anything that touches the meetings
//! folder runs off the main thread: the first access can wait on macOS's
//! Documents permission prompt.

use std::io::Write;
use std::process::{Command, Stdio};

use tauri::AppHandle;

use crate::library::{self, Details, Item};
use crate::processing::{self, Progress};
use crate::recording::{self, Choices, Defaults, Live, SourceList};

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

#[tauri::command]
pub async fn write_summary(app: AppHandle, folder: String) -> Result<(), String> {
    blocking(move || {
        processing::summarize(&app, &library::checked_folder(&folder)?);
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

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}
