//! macOS host: accessory app, native frost, WKWebView pin, AppKit live resize.

mod live_resize;
mod vibrancy;
mod viewport;

use tauri::{AppHandle, WebviewWindow, Window, WindowEvent};

use crate::host::PanelSize;
use crate::platform::Host;

/// Injected by [`crate::platform::current`] on macOS.
pub struct Macos;

impl Host for Macos {
    fn configure_app(&self, app: &AppHandle) {
        // No Dock icon and no app menu: Nessa lives in the menu bar and is
        // summoned from there.
        if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
            eprintln!("[nessa] could not set accessory activation policy: {error}");
        }
    }

    fn set_frosted(&self, window: &WebviewWindow, frosted: bool) -> Result<(), String> {
        vibrancy::set(window, frosted)
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

    fn on_window_event(&self, window: &Window, event: &WindowEvent) {
        // A menu bar extra dismisses when you click away — but not in a debug
        // build, where opening devtools would hide it instantly.
        #[cfg(not(debug_assertions))]
        if let WindowEvent::Focused(false) = event {
            let _ = window.hide();
        }
        #[cfg(debug_assertions)]
        {
            let _ = (window, event);
        }
    }
}
