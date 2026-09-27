//! The global accelerator that summons the panel from anywhere.
//!
//! Binding comes from the stage-scoped `shortcuts.json` cache (`panel.summon`),
//! not from settings ([ADR 0004](../../docs/adr/done/0004-server-owned-keybindings.md)).

use tauri::{AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::host::SUMMONED;
use crate::panel;
use crate::shortcuts::SummonRegistration;

/// Registers the accelerator. A shortcut that cannot be parsed or is already
/// claimed by another app is reported and skipped: the tray icon still opens
/// the panel, so a bad key must not take the app down with it.
pub fn register(app: &AppHandle, accelerator: &str) {
    let shortcut: Shortcut = match accelerator.parse() {
        Ok(shortcut) => shortcut,
        Err(error) => {
            eprintln!("[nessa] {accelerator:?} is not a usable shortcut: {error}");
            return;
        }
    };

    let registered = app
        .global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            // Both edges are delivered; acting on the release too would toggle the
            // panel straight back closed.
            if event.state() == ShortcutState::Pressed {
                // The press was taken by the system, so no window saw a key.
                // Setup teaches this shortcut and has to be able to tell that
                // it worked, which it cannot do from a key it never receives.
                // No panel to toggle is not a press that hid one: it stays
                // silent rather than teaching a lesson that did not happen.
                if let Some(showing) = panel::toggle(app) {
                    let _ = app.emit(SUMMONED, showing);
                }
            }
        });

    if let Err(error) = registered {
        eprintln!("[nessa] could not register {accelerator:?}: {error}");
    }
}

/// Drop the previous summon binding (if any) and register `next` when present.
pub fn reregister_summon(app: &AppHandle, registration: &SummonRegistration, next: Option<&str>) {
    let mut slot = registration
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(previous) = slot.take() {
        if let Err(error) = app.global_shortcut().unregister(previous.as_str()) {
            eprintln!("[nessa] could not unregister {previous:?}: {error}");
        }
    }
    let Some(accelerator) = next else {
        return;
    };
    register(app, accelerator);
    *slot = Some(accelerator.to_string());
}
