//! Debrief desktop app: the window, the menu bar icon, the global shortcuts and
//! the commands the Svelte UI calls.

mod commands;
mod hotkey;
mod library;
mod notify;
mod permissions;
mod phase;
mod processing;
mod recording;
mod relaunch;
mod settings;
mod speech_models;
mod storage;
mod trash;
mod tray;
mod updates;
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(hotkey::plugin(start_or_stop, mark))
        .manage(recording::Current::default())
        .manage(processing::Queue::default())
        .manage(updates::Updates::default())
        .manage(speech_models::Downloads::default())
        .manage(storage::Cleaner::default())
        .menu(app_menu)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if let Some(action) = Action::from_id(id) {
                on_action(app, action);
            } else if let Some(folder) = id.strip_prefix(tray::MEETING_ID_PREFIX) {
                window::open_meeting(app, folder);
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
            commands::meetings,
            commands::meeting,
            commands::process_meeting,
            commands::write_summary,
            commands::stop_processing,
            commands::processing_progress,
            commands::show_in_finder,
            commands::delete_meeting,
            commands::copy_text,
            commands::settings,
            commands::save_settings,
            commands::set_cleanup,
            commands::choose_meetings_folder,
            commands::summary_models,
            commands::use_summary_model,
            commands::edit_summary_prompt,
            commands::permissions,
            commands::request_permission,
            commands::meetings_folder_access,
            commands::open_privacy_settings,
            commands::relaunch,
            commands::update_status,
            commands::check_for_updates,
            commands::install_update,
            commands::speech_models,
            commands::download_model,
            commands::cancel_model_download,
            commands::choose_model,
            commands::delete_model,
        ])
        .setup(|app| {
            let app = app.handle();
            tray::create(app)?;
            phase::set(app, Phase::Idle);
            // Reading the meetings folder can wait on macOS's Documents permission prompt.
            in_background(app, tray::refresh_recent);
            hotkey::register_start_or_stop(app);
            updates::start(app);
            storage::start(app);
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
        let check_updates = MenuItem::with_id(
            app,
            Action::CheckUpdates.id(),
            "Check for Updates…",
            true,
            None::<&str>,
        )?;
        // Right after About, as in other macOS apps.
        app_submenu.insert(&check_updates, 1)?;
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
        Action::CheckUpdates => {
            window::show(app, Some(Screen::Settings));
            let app = app.clone();
            tauri::async_runtime::spawn(async move { updates::check(&app).await });
        }
        Action::Quit => app.exit(0),
    }
}

/// For the global shortcut and for Start and Stop in the menu.
fn start_or_stop(app: &AppHandle) {
    in_background(app, |app| {
        if recording::is_active(app) {
            recording::stop(app, true);
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
