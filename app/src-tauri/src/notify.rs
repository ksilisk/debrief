//! System notifications.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Tells the user that a meeting's files are ready, since processing often ends while they work elsewhere.
pub fn meeting_ready(app: &AppHandle, meeting_title: &str, summary: bool) {
    let shown = app
        .notification()
        .builder()
        .title(meeting_title)
        .body(if summary {
            "The summary is ready."
        } else {
            "The transcript and files are ready."
        })
        .show();
    if let Err(error) = shown {
        eprintln!("Could not show a notification: {error}");
    }
}
