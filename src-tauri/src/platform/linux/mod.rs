//! Linux host: taskbar window, CSS frost, GtkFixed pin, allocate-based resize.

mod live_resize;
mod viewport;
mod webkit;

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

    fn after_attach(&self, window: &WebviewWindow, settings: &Settings) {
        // Linux has no menu bar extra to discover the panel from, so it opens
        // on launch and stays on the taskbar.
        let _ = window.set_skip_taskbar(false);
        crate::panel::show(window, settings);
    }
}
