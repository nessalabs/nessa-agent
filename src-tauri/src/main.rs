// The release build is a menu bar app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod host;
mod local_data;
mod panel;
mod platform;
mod settings;
mod shortcut;
mod tray;

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
        ])
        .setup(|app| {
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
            shortcut::register(app.handle(), &settings.toggle_shortcut);

            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = panel::apply_configured_size(&window, &settings) {
                    eprintln!("[nessa] could not size the panel: {error}");
                }
                platform::bind_window(&window, &settings);
            }

            // The panel reads these on every show, to re-fit the frame.
            app.manage(settings);

            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // The window is the whole app, so closing it means dismissing the
                // panel rather than tearing the process down.
                WindowEvent::CloseRequested { api, .. } => {
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
                _ => {}
            }
            platform::current().on_window_event(window, event);
        })
        .run(tauri::generate_context!())
        .expect("error while running Nessa");
}
