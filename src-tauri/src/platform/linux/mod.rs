//! Linux host: taskbar window, CSS frost, GtkFixed pin, allocate-based resize.

mod live_resize;
mod viewport;
mod webkit;

use std::{process::Command, thread};

use tauri::WebviewWindow;

use crate::host::PanelSize;
use crate::platform::Host;
use crate::settings::Settings;

/// Injected by [`crate::platform::current`] on Linux.
pub struct Linux;

impl Host for Linux {
    fn prepare(&self) {
        webkit::prepare();
    }

    fn panel_size(&self, window: &WebviewWindow) -> Result<PanelSize, String> {
        viewport::panel_size(window)
    }

    fn fit_viewport(&self, window: &WebviewWindow) -> Result<(), String> {
        viewport::fit(window)
    }

    fn watch_viewport(&self, window: &WebviewWindow) -> Result<(), String> {
        viewport::watch(window)
    }

    fn watch_live_resize(&self, window: &WebviewWindow) -> Result<(), String> {
        live_resize::watch(window)
    }

    fn flush_compositor(&self, window: &WebviewWindow) -> Result<(), String> {
        viewport::flush(window)
    }

    fn after_attach(&self, window: &WebviewWindow, settings: &Settings) {
        // Linux has no menu bar extra to discover the panel from, so it opens
        // on launch and stays on the taskbar.
        let _ = window.set_skip_taskbar(false);
        if let Err(error) = crate::panel::show(window, settings) {
            eprintln!("[nessa] could not open the panel on launch: {error}");
        }
    }

    /// `xdg-open` is the desktop-agnostic handler every Linux desktop honours,
    /// and it is looked up on `PATH` rather than by absolute path because
    /// distributions do not agree on where it lives. Spawned and left, like
    /// the macOS opener: the panel does not wait for a browser to start.
    fn open_externally(&self, url: &str) -> Result<(), String> {
        let mut child = Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|error| error.to_string())?;
        // Reaped on a thread of its own. The opener returns as soon as it has
        // told the other app, but waiting here would hold the navigation
        // decision — and so the window — while a cold browser starts. Left
        // unwaited it would be a zombie per click, for the life of a menu bar
        // app that runs for days.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}
