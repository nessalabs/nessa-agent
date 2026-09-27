//! The desktop window: Nessa's main app window, beside the menu bar panel.
//!
//! It is declared hidden in `tauri.conf.json` and opened here. While it is on
//! screen the app has a Dock icon and a place in the app switcher; dismissing
//! it hides it and returns the app to the menu bar alone, the way Alfred's
//! preferences window comes and goes. The panel is unaffected either way.

use tauri::{AppHandle, Manager, Window};

use crate::platform;

/// The window's label, shared with `tauri.conf.json` and `capabilities/desktop.json`.
pub const DESKTOP_WINDOW: &str = "desktop";

/// Shows the desktop window and brings it forward, giving the app its Dock icon.
pub fn open(app: &AppHandle) {
    let Some(window) = app.get_webview_window(DESKTOP_WINDOW) else {
        eprintln!("[nessa] there is no desktop window to open");
        return;
    };
    // Dock presence first: an Accessory app cannot become the active app, so
    // focusing the window before the policy changes would leave it behind.
    platform::current().set_dock_presence(app, true);
    if let Err(error) = window.show() {
        eprintln!("[nessa] could not show the desktop window: {error}");
        return;
    }
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Hides the desktop window instead of destroying it, so it can open again,
/// and takes the Dock icon away with it.
pub fn dismiss(window: &Window) {
    if let Err(error) = window.hide() {
        eprintln!("[nessa] could not hide the desktop window: {error}");
        return;
    }
    platform::current().set_dock_presence(window.app_handle(), false);
}
