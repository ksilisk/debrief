//! Global shortcuts that work from any app: Control+Command+R starts or stops a
//! recording, and Control+Command+M marks a moment while recording.

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Shown next to the menu items.
pub const START_OR_STOP_KEYS: &str = "Ctrl+Cmd+R";
pub const MARK_KEYS: &str = "Ctrl+Cmd+M";

fn start_or_stop() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyR)
}

fn mark() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyM)
}

/// The plugin that receives the shortcuts and calls the matching function.
pub fn plugin(on_start_or_stop: fn(&AppHandle), on_mark: fn(&AppHandle)) -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, shortcut, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            if *shortcut == start_or_stop() {
                on_start_or_stop(app);
            } else if *shortcut == mark() {
                on_mark(app);
            }
        })
        .build()
}

/// Another app may already own the shortcut; Debrief then runs without it.
pub fn register_start_or_stop(app: &AppHandle) {
    register(app, start_or_stop(), "Control+Command+R");
}

/// Only while recording, so other apps keep these keys the rest of the time.
pub fn register_mark(app: &AppHandle) {
    register(app, mark(), "Control+Command+M");
}

pub fn unregister_mark(app: &AppHandle) {
    if let Err(error) = app.global_shortcut().unregister(mark()) {
        eprintln!("Could not release Control+Command+M: {error}");
    }
}

fn register(app: &AppHandle, shortcut: Shortcut, name: &str) {
    if let Err(error) = app.global_shortcut().register(shortcut) {
        eprintln!("{name} is not available: {error}");
    }
}
