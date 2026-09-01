//! Fallback host (Windows and anything that is not macOS or Linux).
//!
//! The webview fills the window. Frost is CSS. There is no native live-resize
//! signal to forward — the page sees the resize gesture itself.

mod viewport;

use tauri::WebviewWindow;

use crate::host::PanelSize;
use crate::platform::Host;

/// Injected by [`crate::platform::current`] off macOS and Linux.
pub struct Other;

impl Host for Other {
    fn panel_size(&self, window: &WebviewWindow) -> Result<PanelSize, String> {
        viewport::panel_size(window)
    }

    fn fit_viewport(&self, window: &WebviewWindow) -> Result<(), String> {
        viewport::fit(window)
    }

    fn watch_viewport(&self, window: &WebviewWindow) -> Result<(), String> {
        viewport::watch(window)
    }
}
