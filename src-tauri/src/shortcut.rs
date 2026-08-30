//! The global accelerator that summons the panel from anywhere.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Registers the accelerator from settings. A shortcut that cannot be parsed
/// or is already claimed by another app is reported and skipped: the tray icon
/// still opens the panel, so a bad key in a hand-edited settings file must not
/// take the app down with it.
pub fn register(app: &AppHandle, accelerator: &str) {
    let shortcut: Shortcut = match accelerator.parse() {
        Ok(shortcut) => shortcut,
        Err(error) => {
            eprintln!("[nessa] {accelerator:?} is not a usable shortcut: {error}");
            return;
        }
    };

    let registered = app.global_shortcut().on_shortcut(shortcut, |app, _, event| {
        // Both edges are delivered; acting on the release too would toggle the
        // panel straight back closed.
        if event.state() == ShortcutState::Pressed {
            crate::tray::toggle_panel(app);
        }
    });

    if let Err(error) = registered {
        eprintln!("[nessa] could not register {accelerator:?}: {error}");
    }
}
