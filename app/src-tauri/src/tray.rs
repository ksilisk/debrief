//! The menu bar icon: how it looks in each phase, and its menu.

use debrief_core::export::clock;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, include_image};

use crate::phase::Phase;

const ID: &str = "main";
const ICON: Image<'_> = include_image!("icons/tray.png");
const RECORDING_ICON: Image<'_> = include_image!("icons/tray-recording.png");

/// Shown next to Start and Stop; the same keys as the global shortcut.
const START_OR_STOP_KEYS: &str = "Ctrl+Cmd+R";

/// What a menu item does. The window's app menu uses the same ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Pause,
    Mark,
    Stop,
    OpenWindow,
    Settings,
    Quit,
}

impl Action {
    const ALL: [Action; 7] = [
        Action::Start,
        Action::Pause,
        Action::Mark,
        Action::Stop,
        Action::OpenWindow,
        Action::Settings,
        Action::Quit,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Pause => "pause",
            Action::Mark => "mark",
            Action::Stop => "stop",
            Action::OpenWindow => "open-window",
            Action::Settings => "settings",
            Action::Quit => "quit",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|action| action.id() == id)
    }
}

#[derive(Debug, PartialEq)]
enum Entry {
    /// A line that only shows status and can't be clicked.
    Status(String),
    Item {
        action: Action,
        label: &'static str,
        keys: Option<&'static str>,
    },
    Separator,
}

fn item(action: Action, label: &'static str, keys: Option<&'static str>) -> Entry {
    Entry::Item {
        action,
        label,
        keys,
    }
}

fn entries(phase: &Phase) -> Vec<Entry> {
    let mut entries = match phase {
        Phase::Idle => vec![
            Entry::Status("Ready to record".into()),
            item(Action::Start, "Start recording", Some(START_OR_STOP_KEYS)),
        ],
        Phase::Recording {
            title,
            elapsed_seconds,
            paused,
        } => {
            let state = if *paused { "Paused" } else { "Recording" };
            vec![
                Entry::Status(format!("{state} · {}", clock(*elapsed_seconds))),
                Entry::Status(title.clone()),
                Entry::Separator,
                item(
                    Action::Pause,
                    if *paused { "Resume" } else { "Pause" },
                    None,
                ),
                item(Action::Mark, "Mark moment", None),
                item(Action::Stop, "Stop and process", Some(START_OR_STOP_KEYS)),
            ]
        }
        Phase::Processing {
            title,
            step,
            percent,
        } => vec![
            Entry::Status(format!("{step} · {percent}%")),
            Entry::Status(title.clone()),
        ],
    };
    entries.extend([
        Entry::Separator,
        item(Action::OpenWindow, "Open Debrief window", None),
        item(Action::Settings, "Settings…", Some("Cmd+,")),
        item(Action::Quit, "Quit Debrief", Some("Cmd+Q")),
    ]);
    entries
}

/// The text next to the icon: the timer while recording, the progress while processing.
fn title(phase: &Phase) -> Option<String> {
    match phase {
        Phase::Idle => None,
        Phase::Recording {
            elapsed_seconds,
            paused: false,
            ..
        } => Some(clock(*elapsed_seconds)),
        Phase::Recording {
            elapsed_seconds,
            paused: true,
            ..
        } => Some(format!("Paused {}", clock(*elapsed_seconds))),
        Phase::Processing { percent, .. } => Some(format!("{percent}%")),
    }
}

/// Adds the icon to the menu bar. [`update`] gives it its menu.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    TrayIconBuilder::with_id(ID)
        .icon(ICON)
        .icon_as_template(true)
        .tooltip(debrief_core::APP_NAME)
        .show_menu_on_left_click(true)
        .build(app)?;
    Ok(())
}

pub fn update(app: &AppHandle, phase: &Phase) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(ID) else {
        return Ok(());
    };
    // Red only while sound is being recorded; otherwise a template icon that follows the menu bar.
    if matches!(phase, Phase::Recording { paused: false, .. }) {
        tray.set_icon_with_as_template(Some(RECORDING_ICON), false)?;
    } else {
        tray.set_icon_with_as_template(Some(ICON), true)?;
    }
    tray.set_title(title(phase))?;
    tray.set_menu(Some(menu(app, &entries(phase))?))?;
    Ok(())
}

fn menu(app: &AppHandle, entries: &[Entry]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for entry in entries {
        match entry {
            Entry::Status(text) => menu.append(&MenuItem::new(app, text, false, None::<&str>)?)?,
            Entry::Item {
                action,
                label,
                keys,
            } => menu.append(&MenuItem::with_id(app, action.id(), label, true, *keys)?)?,
            Entry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok(menu)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(phase: &Phase) -> Vec<Action> {
        entries(phase)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Item { action, .. } => Some(action),
                _ => None,
            })
            .collect()
    }

    fn recording(paused: bool) -> Phase {
        Phase::Recording {
            title: "Sprint planning".into(),
            elapsed_seconds: 1421.6,
            paused,
        }
    }

    #[test]
    fn idle_menu_offers_start() {
        assert_eq!(
            entries(&Phase::Idle)[0],
            Entry::Status("Ready to record".into())
        );
        assert_eq!(
            actions(&Phase::Idle),
            [
                Action::Start,
                Action::OpenWindow,
                Action::Settings,
                Action::Quit
            ]
        );
        assert_eq!(title(&Phase::Idle), None);
    }

    #[test]
    fn recording_menu_shows_time_and_controls() {
        let phase = recording(false);
        let entries = entries(&phase);
        assert_eq!(entries[0], Entry::Status("Recording · 00:23:41".into()));
        assert_eq!(entries[1], Entry::Status("Sprint planning".into()));
        assert_eq!(
            actions(&phase),
            [
                Action::Pause,
                Action::Mark,
                Action::Stop,
                Action::OpenWindow,
                Action::Settings,
                Action::Quit
            ]
        );
        assert_eq!(title(&phase).as_deref(), Some("00:23:41"));
    }

    #[test]
    fn paused_recording_offers_resume() {
        let phase = recording(true);
        let entries = entries(&phase);
        assert_eq!(entries[0], Entry::Status("Paused · 00:23:41".into()));
        assert!(entries.contains(&item(Action::Pause, "Resume", None)));
        assert_eq!(title(&phase).as_deref(), Some("Paused 00:23:41"));
    }

    #[test]
    fn processing_menu_shows_progress() {
        let phase = Phase::Processing {
            title: "Sprint planning".into(),
            step: "Transcribing".into(),
            percent: 61,
        };
        assert_eq!(
            entries(&phase)[0],
            Entry::Status("Transcribing · 61%".into())
        );
        assert_eq!(
            actions(&phase),
            [Action::OpenWindow, Action::Settings, Action::Quit]
        );
        assert_eq!(title(&phase).as_deref(), Some("61%"));
    }

    #[test]
    fn action_ids_round_trip() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("unknown"), None);
    }
}
