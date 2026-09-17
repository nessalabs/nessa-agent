// The release build is a menu bar app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gateway;
mod host;
mod local_data;
mod panel;
mod platform;
mod settings;
mod shortcut;
mod shortcuts;
mod surface_credential;
mod tray;

use gateway::application::Gateway;
use std::sync::Mutex;

use tauri::{Manager, WindowEvent};

fn main() {
    // Before Tauri (and GTK) start. Linux uses this to disable WebKit's
    // DMA-BUF renderer when there is no DRM device.
    platform::current().prepare();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            platform::set_frosted,
            platform::panel_size,
            platform::flush_compositor,
            panel::reveal_setup_window,
            panel::summon_panel,
            surface_credential::load_surface_credential,
            shortcuts::load_shortcuts,
            shortcuts::apply_shortcuts,
        ])
        .setup(|app| {
            app.manage(surface_credential::SurfaceCredential::from_environment());
            if !cfg!(debug_assertions) {
                let runtime = app.path().resource_dir()?.join("runtime");
                app.manage(Gateway::bootstrap(
                    gateway::infrastructure::current(),
                    runtime,
                    local_data::process_stage(),
                ));
            }
            platform::current().configure_app(app.handle());

            // A missing tray is survivable. On Linux especially, GNOME without
            // an app-indicator extension has no tray at all; the panel then
            // has to be reachable from the taskbar.
            let tray_present = match tray::create(app.handle()) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("[nessa] could not create the tray: {error}");
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.set_skip_taskbar(false);
                    }
                    false
                }
            };
            app.manage(tray::Present(tray_present));

            let settings = settings::load(app.handle());
            let shortcut_doc = shortcuts::load(app.handle());
            let summon = shortcuts::SummonRegistration(Mutex::new(None));
            shortcut::reregister_summon(
                app.handle(),
                &summon,
                shortcuts::summon_accelerator(&shortcut_doc),
            );
            app.manage(summon);

            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = panel::apply_configured_size(&window, &settings) {
                    eprintln!("[nessa] could not size the panel: {error}");
                }
                platform::bind_window(&window, &settings);
            }

            // Setup is a takeover: it covers the screen, menu bar included, and
            // is the active window when it does. It is placed while still
            // hidden and shown by its own page, once that page has a frame to
            // show — see `panel::reveal_setup_window`.
            panel::open_setup_window(app.handle());

            // The panel reads these on every show, to re-fit the frame.
            app.manage(settings);

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // With a tray, close dismisses the panel. Without one — Linux
                // with no StatusNotifierItem — close has to end the process,
                // or there is no quit path at all.
                let tray = window
                    .app_handle()
                    .try_state::<tray::Present>()
                    .map(|state| state.0)
                    .unwrap_or(false);
                if tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // The panel is lifted over setup while setup is on screen. If it is
            // still showing when setup goes, it would be left floating above
            // the menu bar for the rest of the session.
            if matches!(event, WindowEvent::Destroyed) && window.label() == panel::SETUP_WINDOW {
                if let Some(panel) = window.app_handle().get_webview_window("main") {
                    platform::current().set_above_overlay(&panel, false);
                }
            }
            platform::current().on_window_event(window, event);
        })
        .build(tauri::generate_context!())
        .expect("error while building Nessa")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(gateway) = app.try_state::<Gateway>() {
                    if settings::load(app).stop_agents_on_quit {
                        if let Err(error) = gateway.stop_agents() {
                            eprintln!("[nessa] could not request agent shutdown: {error}");
                        }
                    }
                }
            }
        });
}
