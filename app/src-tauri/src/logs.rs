//! The log file for bug reports, `~/Library/Logs/Debrief/debrief.log`, with one
//! older file kept. It never gets transcript or summary text. Meeting folders
//! appear by their date and time only, and the home folder as `~`, so a meeting
//! folder in a message goes last or before a colon.

use std::path::PathBuf;
use std::process::Command;

use chrono::Local;
use debrief_core::meeting;
use log::LevelFilter;
use tauri::Runtime;
use tauri::plugin::TauriPlugin;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

const FILE_NAME: &str = "debrief";
const MAX_FILE_BYTES: u128 = 2_000_000;

pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut builder = tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::Stderr))
        .level(LevelFilter::Warn)
        .level_for(env!("CARGO_CRATE_NAME"), LevelFilter::Info)
        .max_file_size(MAX_FILE_BYTES)
        .rotation_strategy(RotationStrategy::KeepSome(1))
        .timezone_strategy(TimezoneStrategy::UseLocal)
        .format(move |out, message, record| {
            let target = record.target();
            let module = target.rsplit("::").next().unwrap_or(target);
            out.finish(format_args!(
                "{} {:<5} {module}: {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                private(&message.to_string(), &home),
            ))
        });
    if let Some(folder) = folder() {
        builder = builder.target(Target::new(TargetKind::Folder {
            path: folder,
            file_name: Some(FILE_NAME.into()),
        }));
    }
    builder.build()
}

pub fn folder() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library/Logs")
            .join(debrief_core::APP_NAME)
    })
}

pub fn file() -> Option<PathBuf> {
    folder().map(|folder| folder.join(format!("{FILE_NAME}.log")))
}

/// The first line of each run, with the versions a bug report needs.
pub fn started() {
    let macos = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    log::info!(
        "Debrief {} started on macOS {macos} ({})",
        debrief_core::VERSION,
        std::env::consts::ARCH
    );
}

/// Logs a panic before the default hook prints it.
pub fn catch_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("{info}");
        default(info);
    }));
}

/// Cuts the title from meeting folders in paths: `…/2026-11-17 14-30 Sprint
/// planning/…` becomes `…/2026-11-17 14-30/…`. Folder names never contain `/`
/// or `:`, so the title ends at the next one.
fn private(text: &str, home: &str) -> String {
    let text = if home.is_empty() {
        text.to_string()
    } else {
        text.replace(home, "~")
    };
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(slash) = rest.find('/') {
        out.push_str(&rest[..=slash]);
        rest = &rest[slash + 1..];
        if let Some(stamp) = meeting::stamp(rest) {
            out.push_str(stamp);
            let end = rest.find(['/', ':']).unwrap_or(rest.len());
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_lose_the_user_name_and_meeting_titles() {
        let text = "could not write /Users/alice/Documents/Debrief/2026-11-17 14-30 Интервью Петров/transcript.md: No space";
        assert_eq!(
            private(text, "/Users/alice"),
            "could not write ~/Documents/Debrief/2026-11-17 14-30/transcript.md: No space"
        );
        assert_eq!(
            private("/m/2026-11-17 14-30 Sync (2): finished in 3 s", ""),
            "/m/2026-11-17 14-30: finished in 3 s"
        );
    }

    #[test]
    fn other_text_stays() {
        let text = "Processing /m/2026-11-17 14-30/meeting.json took 2026-11-17 14-30 Sync";
        assert_eq!(private(text, "/Users/alice"), text);
    }
}
