//! What the app is doing right now. The menu bar icon follows it.

use tauri::AppHandle;

use crate::tray;

#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "processing (debrief-15) starts the Processing phase"
    )
)]
pub enum Phase {
    #[default]
    Idle,
    Recording {
        title: String,
        elapsed_seconds: f64,
        paused: bool,
    },
    Processing {
        title: String,
        /// The step that runs now, such as "Transcribing".
        step: String,
        percent: u8,
    },
}

pub fn set(app: &AppHandle, phase: Phase) {
    if let Err(error) = tray::update(app, &phase) {
        eprintln!("Could not update the menu bar icon: {error}");
    }
}
