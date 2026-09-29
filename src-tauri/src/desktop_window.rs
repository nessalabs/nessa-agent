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

/// What closing the desktop window does, by whether there is a menu bar item
/// to open it again and whether the platform has a Dock to reopen it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnClose {
    /// Hidden, Dock icon and all: the menu bar item opens it again.
    Dismiss,
    /// Hidden, its Dock icon kept: clicking the Dock (`RunEvent::Reopen`)
    /// opens it again, and the app menu can still quit.
    Hide,
    /// Nothing could open it again, so the app quits rather than run on with
    /// no window and no way back.
    Quit,
}

/// Decides what closing the desktop window does. A window let close is gone
/// for good — `open` finds only windows that exist — so it is never let close
/// while the app runs on.
pub fn on_close(tray: bool, dock: bool) -> OnClose {
    match (tray, dock) {
        (true, _) => OnClose::Dismiss,
        (false, true) => OnClose::Hide,
        (false, false) => OnClose::Quit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_a_tray_it_is_dismissed() {
        assert_eq!(on_close(true, true), OnClose::Dismiss);
        assert_eq!(on_close(true, false), OnClose::Dismiss);
    }

    #[test]
    fn without_a_tray_the_dock_can_reopen_it_so_it_is_only_hidden() {
        assert_eq!(on_close(false, true), OnClose::Hide);
    }

    #[test]
    fn without_a_tray_or_a_dock_nothing_could_reopen_it_so_the_app_quits() {
        assert_eq!(on_close(false, false), OnClose::Quit);
    }
}
