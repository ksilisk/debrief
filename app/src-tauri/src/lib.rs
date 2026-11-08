//! Debrief desktop app: the window, the menu bar icon, the global shortcuts and
//! the commands the Svelte UI calls.

mod commands;
mod hotkey;
mod notify;
mod phase;
mod recording;
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
        .plugin(hotkey::plugin(start_or_stop, mark))
        .manage(recording::Current::default())
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
        .invoke_handler(tauri::generate_handler![
            commands::recording_defaults,
            commands::recording_sources,
            commands::current_recording,
            commands::start_recording,
            commands::toggle_pause,
            commands::mark_moment,
            commands::stop_recording,
            commands::show_in_finder,
        ])
        .setup(|app| {
            let app = app.handle();
            tray::create(app)?;
            phase::set(app, Phase::Idle);
            hotkey::register_start_or_stop(app);
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("{} failed to start: {error}", debrief_core::APP_NAME));

    app.run(|app, event| match event {
        // Quit in both menus and Command+Q.
        RunEvent::ExitRequested { api, .. } if recording::needs_saving(app) => {
            api.prevent_exit();
            in_background(app, |app| {
                recording::save_before_quit(app);
                app.exit(0);
            });
        }
        RunEvent::Exit => recording::save_at_exit(app),
        // A click on the Dock icon while the window is hidden.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => window::show(app, None),
        _ => {}
    });
}

/// The default app menu, plus Settings… under the app name and Debrief's own Quit.
fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    #[cfg(target_os = "macos")]
    if let Some(app_submenu) = menu.items()?.first().and_then(|item| item.as_submenu()) {
        use tauri::menu::{MenuItem, PredefinedMenuItem};
        let settings =
            MenuItem::with_id(app, Action::Settings.id(), "Settings…", true, Some("Cmd+,"))?;
        // Right after About and its separator, where macOS apps keep it.
        app_submenu.insert_items(&[&settings, &PredefinedMenuItem::separator(app)?], 2)?;
        // The built-in Quit ends the app at once; this one saves a recording first.
        if let Some(last) = app_submenu.items()?.len().checked_sub(1) {
            app_submenu.remove_at(last)?;
        }
        let quit = MenuItem::with_id(app, Action::Quit.id(), "Quit Debrief", true, Some("Cmd+Q"))?;
        app_submenu.append(&quit)?;
    }
    Ok(menu)
}

fn on_action(app: &AppHandle, action: Action) {
    match action {
        Action::Start | Action::Stop => start_or_stop(app),
        Action::Pause => in_background(app, |app| {
            if let Err(error) = recording::toggle_pause(app) {
                eprintln!("Could not pause or resume: {error}");
            }
        }),
        Action::Mark => mark(app),
        Action::OpenWindow => window::show(app, None),
        Action::Settings => window::show(app, Some(Screen::Settings)),
        Action::Quit => app.exit(0),
    }
}

/// For the global shortcut and for Start and Stop in the menu.
fn start_or_stop(app: &AppHandle) {
    in_background(app, |app| {
        if recording::is_active(app) {
            recording::stop(app);
        } else {
            recording::start_with_defaults(app);
        }
    });
}

fn mark(app: &AppHandle) {
    in_background(app, |app| {
        if let Err(error) = recording::mark(app) {
            eprintln!("Could not mark the moment: {error}");
        }
    });
}

/// Starting, stopping and pausing can take a moment, so they never run on the main thread.
fn in_background(app: &AppHandle, work: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || work(&app));
}
