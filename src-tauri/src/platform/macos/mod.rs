//! macOS host: accessory app, native frost, WKWebView pin, AppKit live resize.

mod live_resize;
mod vibrancy;
mod viewport;

use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
use tauri::{AppHandle, WebviewWindow};

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

    fn after_attach(&self, window: &WebviewWindow, _settings: &crate::settings::Settings) {
        // Setup runs on the main thread. The live WebviewWindow owns this
        // NSWindow; borrow it only for the duration of this configuration.
        let handle = match window.ns_window() {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("[nessa] could not configure fullscreen Space visibility: {error}");
                return;
            }
        };
        let native = unsafe { &*handle.cast::<NSWindow>() };
        // Tauri's all-workspaces flag joins desktop Spaces. This additional
        // behavior allows the accessory panel beside fullscreen windows.
        native.setCollectionBehavior(
            native.collectionBehavior() | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
    }
}
