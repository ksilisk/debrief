//! Debrief desktop app: the window, the menu bar icon, the global shortcut and
//! the commands the Svelte UI calls.

mod hotkey;
mod notify;
mod phase;
mod tray;
mod window;

use tauri::menu::Menu;
use tauri::{AppHandle, RunEvent, WindowEvent, Wry};

use phase::Phase;
use tray::Action;
use window::Screen;

/// Starts the app and blocks until it quits.
pub fn run() {
    let app = tauri::Builder::default()
        // Must come first: a second copy of the app hands over to this one and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            window::show(app, None);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(hotkey::plugin(start_or_stop))
        .manage(phase::Current::default())
        .menu(app_menu)
        .on_menu_event(|app, event| {
            if let Some(action) = Action::from_id(event.id().as_ref()) {
                on_action(app, action);
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    eprintln!("Could not hide the window: {error}");
                }
            }
        })
        .setup(|app| {
            let app = app.handle();
            tray::create(app)?;
            phase::set(app, Phase::Idle);
            hotkey::register(app);
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("{} failed to start: {error}", debrief_core::APP_NAME));

    app.run(|app, event| {
        // A click on the Dock icon while the window is hidden.
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            window::show(app, None);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
}

/// The default app menu, plus Settings… under the app name.
fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    #[cfg(target_os = "macos")]
    if let Some(app_submenu) = menu.items()?.first().and_then(|item| item.as_submenu()) {
        use tauri::menu::{MenuItem, PredefinedMenuItem};
        let settings =
            MenuItem::with_id(app, Action::Settings.id(), "Settings…", true, Some("Cmd+,"))?;
        // Right after About and its separator, where macOS apps keep it.
        app_submenu.insert_items(&[&settings, &PredefinedMenuItem::separator(app)?], 2)?;
    }
    Ok(menu)
}

fn on_action(app: &AppHandle, action: Action) {
    match action {
        Action::Start | Action::Stop => start_or_stop(app),
        // The menu shows these only while recording, which starts with debrief-14.
        Action::Pause | Action::Mark => window::show(app, Some(Screen::Recording)),
        Action::OpenWindow => window::show(app, None),
        Action::Settings => window::show(app, Some(Screen::Settings)),
        Action::Quit => app.exit(0),
    }
}

/// Runs for the global shortcut and for Start and Stop in the menu. Until the
/// recorder is connected (debrief-14), it only opens the matching screen.
fn start_or_stop(app: &AppHandle) {
    let screen = match phase::get(app) {
        Phase::Idle => Screen::New,
        Phase::Recording { .. } => Screen::Recording,
        Phase::Processing { .. } => Screen::Processing,
    };
    window::show(app, Some(screen));
}
