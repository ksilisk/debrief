//! The menu bar icon: how it looks in each phase, and its menu.

use std::sync::{Mutex, PoisonError};

use debrief_core::export::clock;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry, include_image};

use crate::hotkey::{MARK_KEYS, START_OR_STOP_KEYS};
use crate::phase::Phase;

const ID: &str = "main";
const ICON: Image<'_> = include_image!("icons/tray.png");
const RECORDING_ICON: Image<'_> = include_image!("icons/tray-recording.png");

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
                item(Action::Mark, "Mark moment", Some(MARK_KEYS)),
                item(Action::Stop, "Stop recording", Some(START_OR_STOP_KEYS)),
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

/// The menu as last shown, so that a new timer value only changes a line of text.
#[derive(Default)]
struct Shown(Mutex<Option<Rendered>>);

struct Rendered {
    entries: Vec<Entry>,
    recording_icon: bool,
    /// One for each [`Entry::Status`], in order.
    status_items: Vec<MenuItem<Wry>>,
}

/// Adds the icon to the menu bar. [`update`] gives it its menu.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    app.manage(Shown::default());
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
    let shown = app.state::<Shown>();
    let mut shown = shown.0.lock().unwrap_or_else(PoisonError::into_inner);
    let entries = entries(phase);

    // Red only while sound is being recorded; otherwise a template icon that follows the menu bar.
    let recording_icon = matches!(phase, Phase::Recording { paused: false, .. });
    if shown
        .as_ref()
        .is_none_or(|rendered| rendered.recording_icon != recording_icon)
    {
        if recording_icon {
            tray.set_icon_with_as_template(Some(RECORDING_ICON), false)?;
        } else {
            tray.set_icon_with_as_template(Some(ICON), true)?;
        }
    }
    tray.set_title(title(phase))?;

    match shown.as_mut() {
        Some(rendered) if same_items(&rendered.entries, &entries) => {
            let changed = rendered
                .entries
                .iter()
                .zip(&entries)
                .filter(|(old, _)| matches!(old, Entry::Status(_)))
                .zip(&rendered.status_items);
            for ((old, new), item) in changed {
                if let (true, Entry::Status(text)) = (old != new, new) {
                    item.set_text(text)?;
                }
            }
            rendered.entries = entries;
            rendered.recording_icon = recording_icon;
        }
        _ => {
            let (menu, status_items) = menu(app, &entries)?;
            tray.set_menu(Some(menu))?;
            *shown = Some(Rendered {
                entries,
                recording_icon,
                status_items,
            });
        }
    }
    Ok(())
}

/// Whether two menus differ at most in the text of their status lines.
fn same_items(old: &[Entry], new: &[Entry]) -> bool {
    old.len() == new.len()
        && old.iter().zip(new).all(|(old, new)| {
            matches!((old, new), (Entry::Status(_), Entry::Status(_))) || old == new
        })
}

fn menu(app: &AppHandle, entries: &[Entry]) -> tauri::Result<(Menu<Wry>, Vec<MenuItem<Wry>>)> {
    let menu = Menu::new(app)?;
    let mut status_items = Vec::new();
    for entry in entries {
        match entry {
            Entry::Status(text) => {
                let item = MenuItem::new(app, text, false, None::<&str>)?;
                menu.append(&item)?;
                status_items.push(item);
            }
            Entry::Item {
                action,
                label,
                keys,
            } => menu.append(&MenuItem::with_id(app, action.id(), label, true, *keys)?)?,
            Entry::Separator => menu.append(&PredefinedMenuItem::separator(app)?)?,
        }
    }
    Ok((menu, status_items))
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
    fn a_new_time_keeps_the_menu_items() {
        let later = Phase::Recording {
            title: "Sprint planning".into(),
            elapsed_seconds: 1422.0,
            paused: false,
        };
        assert!(same_items(&entries(&recording(false)), &entries(&later)));
        assert!(!same_items(
            &entries(&recording(false)),
            &entries(&recording(true))
        ));
        assert!(!same_items(&entries(&Phase::Idle), &entries(&later)));
    }

    #[test]
    fn action_ids_round_trip() {
        for action in Action::ALL {
            assert_eq!(Action::from_id(action.id()), Some(action));
        }
        assert_eq!(Action::from_id("unknown"), None);
    }
}
