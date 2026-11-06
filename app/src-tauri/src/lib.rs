//! Debrief desktop app: the window, and later the menu bar icon, hotkey and
//! the commands the Svelte UI calls (M2).

/// Starts the app and blocks until it quits.
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("{} failed to start: {error}", debrief_core::APP_NAME));
}
