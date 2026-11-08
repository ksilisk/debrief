//! The global shortcut Control+Command+R: start or stop a recording from any app.

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Wry};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

fn start_or_stop() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SUPER), Code::KeyR)
}

/// The plugin that receives the shortcut; `on_press` runs each time it is pressed.
pub fn plugin(on_press: fn(&AppHandle)) -> TauriPlugin<Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, shortcut, event| {
            if *shortcut == start_or_stop() && event.state() == ShortcutState::Pressed {
                on_press(app);
            }
        })
        .build()
}

/// Another app may already own the shortcut; Debrief then runs without it.
pub fn register(app: &AppHandle) {
    if let Err(error) = app.global_shortcut().register(start_or_stop()) {
        eprintln!("Control+Command+R is not available: {error}");
    }
}
