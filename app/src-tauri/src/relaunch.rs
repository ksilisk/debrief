//! Quitting and opening Debrief again, after an update or after screen
//! recording is turned on (it only works in a new process).

use std::process::Command;

use tauri::AppHandle;

use crate::{processing, recording};

pub fn relaunch(app: &AppHandle) -> Result<(), String> {
    if recording::is_active(app) {
        return Err("Stop the recording before Debrief quits and opens again.".into());
    }
    if processing::is_busy(app) {
        return Err("Wait until processing finishes before Debrief quits and opens again.".into());
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let bundle = exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .ok_or("Debrief isn't running from Debrief.app, so open it again yourself.")?;
    // The new copy waits until this one has quit; otherwise it would hand over and exit.
    Command::new("/bin/sh")
        .arg("-c")
        .arg("sleep 1; /usr/bin/open \"$0\"")
        .arg(bundle)
        .spawn()
        .map_err(|error| format!("Could not open Debrief again: {error}"))?;
    app.exit(0);
    Ok(())
}
