//! The Permissions screen: what macOS allows Debrief, the system prompts and
//! the System Settings pages.

use std::fs;
use std::io;
use std::process::Command;

use debrief_capture::{Access, Permission};
use debrief_summarize::config;
use serde::{Deserialize, Serialize};

use crate::settings::with_tilde;

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
