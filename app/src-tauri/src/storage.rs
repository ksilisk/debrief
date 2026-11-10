//! Storage cleanup: after the number of days chosen in Settings, old meetings'
//! audio and video, or whole meetings, go to the Trash. Debrief checks a minute
//! after launch, every 12 hours, and when the setting changes. Moving one
//! meeting to the Trash by hand goes through here too.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local};
use debrief_core::cleanup::{self, Delete, Due};
use debrief_core::meeting::Meeting;
use debrief_core::session::is_recording;
use debrief_summarize::config::{self, CleanupSettings, Settings};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::processing::{self, MEETINGS_CHANGED};
use crate::settings::failed;
use crate::{library, trash, tray, window};

/// The first run waits so it doesn't slow down the start.
const FIRST_RUN: Duration = Duration::from_secs(60);
const RUN_EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// Kept in Tauri's managed state, so only one cleanup runs at a time.
#[derive(Default)]
pub struct Cleaner(Mutex<()>);

pub fn start(app: &AppHandle) {
    let app = app.clone();
    thread::spawn(move || {
        thread::sleep(FIRST_RUN);
        loop {
            run(&app);
            thread::sleep(RUN_EVERY);
        }
    });
}

/// Applies the saved setting without asking.
fn run(app: &AppHandle) {
    let cleaner = app.state::<Cleaner>();
    let _one = lock(&cleaner.0);
    let settings = match config::folder().and_then(|folder| Settings::load(&folder)) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("Storage cleanup: {error}");
            return;
        }
    };
    let (Some(rule), Ok(library)) = (settings.cleanup.rule(), settings.library()) else {
        return;
    };
    let due = cleanup::due(&library, rule, now(), &|folder| is_busy(app, folder));
    if let Some(problem) = apply(app, &due) {
        eprintln!("Storage cleanup: {problem}");
    }
}

/// Saves the setting. When it applies to meetings now, asks first; `false`
/// when the answer was no and nothing changed.
pub fn change(app: &AppHandle, choice: CleanupSettings) -> Result<bool, String> {
    let cleaner = app.state::<Cleaner>();
    let _one = lock(&cleaner.0);
    let folder = config::folder().map_err(failed)?;
    let mut settings = Settings::load(&folder).map_err(failed)?;
    let due = match choice.rule() {
        Some(rule) => {
            let library = settings.library().map_err(failed)?;
            cleanup::due(&library, rule, now(), &|folder| is_busy(app, folder))
        }
        None => Vec::new(),
    };
    if let (Some(rule), false) = (choice.rule(), due.is_empty()) {
        let (question, detail) = question(&due, rule.delete, rule.after_days);
        if !confirm(app, &question, &detail) {
            return Ok(false);
        }
    }
    settings.cleanup = choice;
    settings.save(&folder).map_err(failed)?;
    match apply(app, &due) {
        Some(problem) => Err(format!("Some files didn't go to the Trash: {problem}")),
        None => Ok(true),
    }
}

/// Moves one meeting folder to the Trash after asking. Problems show in an
/// alert; `false` when nothing was moved.
pub fn delete_meeting(app: &AppHandle, folder: &Path) -> bool {
    let cleaner = app.state::<Cleaner>();
    let _one = lock(&cleaner.0);
    let title = Meeting::load(folder)
        .map(|meeting| library::title(folder, &meeting))
        .unwrap_or_else(|_| folder.display().to_string());
    if is_recording(folder) {
        alert(
            app,
            &format!("“{title}” is being recorded"),
            "Stop the recording first.",
        );
        return false;
    }
    if is_busy(app, folder) {
        alert(
            app,
            &format!("“{title}” is being processed"),
            "Stop processing it or take it out of the queue first.",
        );
        return false;
    }
    let question = format!("Move “{title}” to the Trash?");
    let detail =
        "The meeting folder goes to the Trash with its recordings, transcript and summary.";
    if !confirm(app, &question, detail) {
        return false;
    }
    if let Err(error) = trash::move_to_trash(folder) {
        alert(
            app,
            &format!("Could not move “{title}” to the Trash"),
            &error.to_string(),
        );
        return false;
    }
    processing::forget(app, folder);
    changed(app);
    true
}

/// The first problem, if any. Meetings that became busy in the meantime wait for the next run.
fn apply(app: &AppHandle, due: &[Due]) -> Option<String> {
    if due.is_empty() {
        return None;
    }
    let now = now();
    let mut problem = None;
    for due in due {
        if is_busy(app, &due.folder) {
            continue;
        }
        if let Err(error) = cleanup::clean(due, now, &mut |path| trash::move_to_trash(path)) {
            problem.get_or_insert(error.to_string());
        }
    }
    changed(app);
    problem
}

/// The question and the detail text for the confirmation.
fn question(due: &[Due], delete: Delete, after_days: u32) -> (String, String) {
    let one = match due {
        [only] => Some(library::title(&only.folder, &only.meeting)),
        _ => None,
    };
    let bytes = due.iter().map(|due| due.bytes).sum();
    let older = match &one {
        Some(_) => format!("The meeting is older than {}.", age(after_days)),
        None => format!("These meetings are older than {}.", age(after_days)),
    };
    let freed = format!("{} is freed when you empty the Trash.", size(bytes));
    match (delete, one) {
        (Delete::Recordings, Some(title)) => (
            format!("Move the audio and video of “{title}” to the Trash?"),
            format!("{older} The transcript and summary stay. {freed}"),
        ),
        (Delete::Recordings, None) => (
            format!(
                "Move the audio and video of {} meetings to the Trash?",
                due.len()
            ),
            format!("{older} Their transcripts and summaries stay. {freed}"),
        ),
        (Delete::Meeting, Some(title)) => (
            format!("Move “{title}” to the Trash?"),
            format!("{older} Its transcript and summary go too. {freed}"),
        ),
        (Delete::Meeting, None) => (
            format!("Move {} meetings to the Trash?", due.len()),
            format!("{older} Their transcripts and summaries go too. {freed}"),
        ),
    }
}

fn age(days: u32) -> String {
    match days {
        1 => "1 day".into(),
        365 => "1 year".into(),
        days => format!("{days} days"),
    }
}

fn size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.0} MB", bytes / 1e6)
    } else {
        format!("{:.0} KB", (bytes / 1e3).max(1.0))
    }
}

fn confirm(app: &AppHandle, question: &str, detail: &str) -> bool {
    let dialog = app
        .dialog()
        .message(detail)
        .title(question)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Move to Trash".into(),
            "Cancel".into(),
        ));
    match app.get_webview_window(window::MAIN) {
        Some(main) => dialog.parent(&main).blocking_show(),
        None => dialog.blocking_show(),
    }
}

fn alert(app: &AppHandle, problem: &str, detail: &str) {
    let dialog = app
        .dialog()
        .message(detail)
        .title(problem)
        .kind(MessageDialogKind::Error);
    match app.get_webview_window(window::MAIN) {
        Some(main) => dialog.parent(&main).blocking_show(),
        None => dialog.blocking_show(),
    };
}

fn is_busy(app: &AppHandle, folder: &Path) -> bool {
    processing::status_of(app, folder).is_some()
}

fn changed(app: &AppHandle) {
    if let Err(error) = app.emit(MEETINGS_CHANGED, ()) {
        eprintln!("Could not refresh the meeting list: {error}");
    }
    tray::refresh_recent(app);
}

fn now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_and_sizes_read_plainly() {
        assert_eq!(age(30), "30 days");
        assert_eq!(age(365), "1 year");
        assert_eq!(size(2_340_000_000), "2.3 GB");
        assert_eq!(size(57_400_000), "57 MB");
        assert_eq!(size(200), "1 KB");
    }
}
