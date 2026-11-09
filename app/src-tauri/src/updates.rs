//! Updates from the GitHub repo's latest release. Debrief checks at launch and
//! every 12 hours, and installs only when asked, never during a recording or
//! processing.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::{processing, recording, relaunch};

const STATUS: &str = "update-status";
/// The first check waits so it doesn't slow down the start.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum State {
    NotChecked,
    Checking,
    UpToDate,
    Available {
        version: String,
    },
    Downloading {
        percent: u8,
    },
    /// Debrief is about to quit and open the new version.
    Installed,
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The running version.
    pub current: String,
    #[serde(flatten)]
    pub state: State,
}

/// Kept in Tauri's managed state.
pub struct Updates {
    state: Mutex<State>,
    found: Mutex<Option<Update>>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::NotChecked),
            found: Mutex::new(None),
        }
    }
}

/// Starts the regular checks. Development builds don't run from an app bundle,
/// so they can't be updated and don't check.
pub fn start(app: &AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(FIRST_CHECK);
        loop {
            tauri::async_runtime::block_on(check(&app));
            thread::sleep(CHECK_EVERY);
        }
    });
}

pub fn status(app: &AppHandle) -> Status {
    Status {
        current: app.package_info().version.to_string(),
        state: lock(&app.state::<Updates>().state).clone(),
    }
}

pub async fn check(app: &AppHandle) {
    let busy = matches!(
        *lock(&app.state::<Updates>().state),
        State::Checking | State::Downloading { .. } | State::Installed
    );
    if busy {
        return;
    }
    set(app, State::Checking);
    let checked = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let state = match checked {
        Ok(Some(update)) => {
            let version = update.version.clone();
            *lock(&app.state::<Updates>().found) = Some(update);
            State::Available { version }
        }
        Ok(None) => State::UpToDate,
        Err(error) => State::Failed {
            message: format!("Could not check for updates: {error}"),
        },
    };
    set(app, state);
}

/// Downloads the update found by the last check, checks its signature, puts it
/// in place of this app and opens it.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    if recording::is_active(app) {
        return Err("Stop the recording first; the update installs after it.".into());
    }
    if processing::is_busy(app) {
        return Err("Wait until processing finishes; the update installs after it.".into());
    }
    let update = lock(&app.state::<Updates>().found)
        .clone()
        .ok_or("Check for updates first.")?;
    set(app, State::Downloading { percent: 0 });
    let mut done = 0usize;
    let mut shown = 0u8;
    let installed = update
        .download_and_install(
            |chunk, total| {
                done += chunk;
                if let Some(total) = total.filter(|&total| total > 0) {
                    let percent = (done as u64 * 100 / total).min(100) as u8;
                    if percent != shown {
                        shown = percent;
                        set(app, State::Downloading { percent });
                    }
                }
            },
            || {},
        )
        .await;
    if let Err(error) = installed {
        let message = format!("Could not install the update: {error}");
        set(
            app,
            State::Failed {
                message: message.clone(),
            },
        );
        return Err(message);
    }
    set(app, State::Installed);
    relaunch::relaunch(app)
}

fn set(app: &AppHandle, state: State) {
    *lock(&app.state::<Updates>().state) = state;
    if let Err(error) = app.emit(STATUS, status(app)) {
        eprintln!("Could not send the update status to the window: {error}");
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
