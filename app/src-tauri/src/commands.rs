//! The commands the Svelte window calls. The slow ones run off the main thread.

use std::path::Path;
use std::process::Command;

use debrief_core::meeting;
use tauri::AppHandle;

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
        recording::stop(&app);
        Ok(())
    })
    .await
}

/// Shows a meeting folder in Finder. Only folders inside the meetings folder.
#[tauri::command]
pub fn show_in_finder(folder: String) -> Result<(), String> {
    let library = meeting::default_library().ok_or("HOME is not set")?;
    let library = library.canonicalize().unwrap_or(library);
    let folder = Path::new(&folder)
        .canonicalize()
        .map_err(|error| format!("Could not find {folder}: {error}"))?;
    if !folder.starts_with(&library) {
        return Err(format!(
            "{} is not in {}",
            folder.display(),
            library.display()
        ));
    }
    Command::new("open")
        .arg("-R")
        .arg(&folder)
        .status()
        .map_err(|error| format!("Could not open Finder: {error}"))?;
    Ok(())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}
