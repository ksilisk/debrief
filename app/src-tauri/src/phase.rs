//! What the app is doing right now. The menu bar icon follows it.

use std::sync::{Mutex, PoisonError};

use tauri::{AppHandle, Manager};

use crate::tray;

#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the recorder (debrief-14) and processing (debrief-15) start the other phases"
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

/// The current phase, kept in Tauri's managed state.
#[derive(Default)]
pub struct Current(Mutex<Phase>);

pub fn get(app: &AppHandle) -> Phase {
    app.state::<Current>()
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

pub fn set(app: &AppHandle, phase: Phase) {
    if let Err(error) = tray::update(app, &phase) {
        eprintln!("Could not update the menu bar icon: {error}");
    }
    *app.state::<Current>()
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = phase;
}
