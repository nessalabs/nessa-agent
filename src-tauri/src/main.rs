// The release build is a menu bar app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod live_resize;
mod settings;
mod shortcut;
mod tray;
mod vibrancy;
mod viewport;

use tauri::{Manager, WindowEvent};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![vibrancy::set_frosted])
        .setup(|app| {
            // No Dock icon and no app menu: Nessa lives in the menu bar and is
            // summoned from there.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            tray::create(app.handle())?;

            let settings = settings::load(app.handle());
            shortcut::register(app.handle(), &settings.toggle_shortcut);

            // The frosted surface is the default; the frontend turns it off if
            // the reader last chose the clear one. A failure here only costs
            // the blur, so it is reported rather than fatal.
            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = tray::apply_configured_size(&window, &settings) {
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
            }

            // The tray reads these on every show, to re-fit the panel.
            app.manage(settings);

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // The window is the whole app, so closing it means dismissing the
            // panel rather than tearing the process down.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            // A menu bar panel dismisses when you click away — but not in a
            // debug build, where opening devtools would hide it instantly.
            #[cfg(not(debug_assertions))]
            WindowEvent::Focused(false) => {
                let _ = window.hide();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Nessa");
}
