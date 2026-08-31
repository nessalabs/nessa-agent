// The release build is a menu bar app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod host;
mod live_resize;
mod panel;
mod settings;
mod shortcut;
mod tray;
mod vibrancy;
mod viewport;

use tauri::{Manager, WindowEvent};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            vibrancy::set_frosted,
            viewport::panel_size
        ])
        .setup(|app| {
            // No Dock icon and no app menu: Nessa lives in the menu bar and is
            // summoned from there.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

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
                // The frosted surface is the default; the frontend turns it off
                // if the reader last chose the clear one. A failure here only
                // costs the blur, so it is reported rather than fatal.
                if let Err(error) = panel::apply_configured_size(&window, &settings) {
                    eprintln!("[nessa] could not size the panel: {error}");
                }
                if let Err(error) = vibrancy::set(&window, true) {
                    eprintln!("[nessa] could not frost the panel: {error}");
                }
                // The webview is sized once, to the work area, and then left
                // alone: a resize that moves the page's viewport is what
                // displaces everything anchored to the bottom of the panel.
                // Failing here costs the fix, not the panel, so it is reported.
                if let Err(error) = viewport::fit(&window) {
                    eprintln!("[nessa] could not fit the panel's viewport: {error}");
                }
                if let Err(error) = viewport::watch(&window) {
                    eprintln!("[nessa] could not watch the panel's viewport: {error}");
                }
                // Only the border glow depends on this, so a failure here is
                // reported rather than fatal.
                if let Err(error) = live_resize::watch(&window) {
                    eprintln!("[nessa] could not watch live resize: {error}");
                }

                // Linux has no menu bar extra to discover the panel from, so
                // it opens on launch and stays on the taskbar. macOS keeps
                // the accessory-app behaviour: hidden until summoned.
                #[cfg(target_os = "linux")]
                {
                    let _ = window.set_skip_taskbar(false);
                    panel::show(&window, &settings);
                }
            }

            // The panel reads these on every show, to re-fit the frame.
            app.manage(settings);

            Ok(())
        })
        .on_window_event(|window, event| match event {
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
            // A menu bar extra dismisses when you click away — but not in a
            // debug build, where opening devtools would hide it instantly.
            // Linux is a normal window on the taskbar; it stays put.
            #[cfg(all(target_os = "macos", not(debug_assertions)))]
            WindowEvent::Focused(false) => {
                let _ = window.hide();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Nessa");
}
