//! The Permissions screen: what macOS allows Debrief, the system prompts, the
//! System Settings pages, and quitting and reopening so a new permission works.

use std::fs;
use std::io;
use std::process::Command;

use debrief_capture::{Access, Permission};
use debrief_summarize::config;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::settings::with_tilde;
use crate::{processing, recording};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Allowed,
    Denied,
    NotAsked,
    Unknown,
}

impl From<Access> for Status {
    fn from(access: Access) -> Self {
        match access {
            Access::Allowed => Status::Allowed,
            Access::Denied => Status::Denied,
            Access::NotAsked => Status::NotAsked,
            Access::Unknown => Status::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub microphone: Status,
    pub screen_recording: Status,
    pub system_audio: Status,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// With `~` for the home folder.
    pub path: String,
    pub status: Status,
}

/// A permission the window asks about, and the System Settings page for it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Microphone,
    ScreenRecording,
    SystemAudio,
    Files,
}

impl Kind {
    fn permission(self) -> Option<Permission> {
        match self {
            Kind::Microphone => Some(Permission::Microphone),
            Kind::ScreenRecording => Some(Permission::ScreenRecording),
            Kind::SystemAudio => Some(Permission::SystemAudio),
            Kind::Files => None,
        }
    }

    fn settings_page(self) -> &'static str {
        match self {
            Kind::Microphone => "Privacy_Microphone",
            Kind::ScreenRecording => "Privacy_ScreenCapture",
            Kind::SystemAudio => "Privacy_AudioCapture",
            Kind::Files => "Privacy_FilesAndFolders",
        }
    }
}

pub fn view() -> View {
    View {
        microphone: debrief_capture::access(Permission::Microphone).into(),
        screen_recording: debrief_capture::access(Permission::ScreenRecording).into(),
        system_audio: debrief_capture::access(Permission::SystemAudio).into(),
    }
}

/// Shows the system prompt if there is one to show, and waits for the answer.
pub fn request(kind: Kind) -> Status {
    match kind.permission() {
        Some(permission) => debrief_capture::request(permission).into(),
        None => folder().status,
    }
}

/// Opening the meetings folder is the check: macOS asks for Documents and
/// similar folders the first time, and this waits for that answer.
pub fn folder() -> Folder {
    let Ok(library) = config::library() else {
        return Folder {
            path: String::new(),
            status: Status::Unknown,
        };
    };
    let opened = fs::create_dir_all(&library).and_then(|()| fs::read_dir(&library));
    let status = match opened {
        Ok(_) => Status::Allowed,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Status::Denied,
        Err(_) => Status::Unknown,
    };
    Folder {
        path: with_tilde(&library),
        status,
    }
}

pub fn open_settings(kind: Kind) -> Result<(), String> {
    let url = format!(
        "x-apple.systempreferences:com.apple.preference.security?{}",
        kind.settings_page()
    );
    Command::new("open")
        .arg(url)
        .status()
        .map_err(|error| format!("Could not open System Settings: {error}"))?;
    Ok(())
}

/// Screen recording starts working only in a new process, so this quits and
/// starts Debrief again from its app bundle.
pub fn relaunch(app: &AppHandle) -> Result<(), String> {
    if recording::is_active(app) {
        return Err("Stop the recording before you quit and reopen Debrief.".into());
    }
    if processing::is_busy(app) {
        return Err("Wait until processing finishes before you quit and reopen Debrief.".into());
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
        .map_err(|error| format!("Could not reopen Debrief: {error}"))?;
    app.exit(0);
    Ok(())
}
