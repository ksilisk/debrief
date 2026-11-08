//! The main window. Closing it only hides it; the app keeps running in the menu bar.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

const MAIN: &str = "main";

/// Tells the window which screen to show. The payload is a [`Screen`].
const NAVIGATE: &str = "navigate";

/// Screens the Rust side can open. Names match `Screen` in `src/lib/screens.ts`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
    New,
    Settings,
}

/// Tells the window to open a meeting. The payload is the meeting folder.
const OPEN_MEETING: &str = "open-meeting";

pub fn open_meeting(app: &AppHandle, folder: &str) {
    show(app, None);
    if let Some(window) = app.get_webview_window(MAIN)
        && let Err(error) = window.emit(OPEN_MEETING, folder)
    {
        eprintln!("Could not open the meeting: {error}");
    }
}

/// Brings the main window to the front, on `screen` when one is given.
pub fn show(app: &AppHandle, screen: Option<Screen>) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let shown = window
        .unminimize()
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus());
    if let Err(error) = shown {
        eprintln!("Could not show the window: {error}");
    }
    if let Some(screen) = screen
        && let Err(error) = window.emit(NAVIGATE, screen)
    {
        eprintln!("Could not open the {screen:?} screen: {error}");
    }
}
