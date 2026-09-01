//! OS-specific host behaviour, injected at compile time.
//!
//! Shared code (`panel`, `tray`, `shortcut`, the shell) never mentions macOS
//! or Linux. It talks to [`Host`]: one trait, one job — the things about the
//! floating window that the OS owns. `current()` is the injector: the binary
//! is built with exactly one implementation, so there is no runtime switch and
//! no `cfg` sprinkled through `main`.
//!
//! | Concern | macOS | Linux | elsewhere |
//! |---|---|---|---|
//! | Frost | `NSVisualEffectView` | CSS `backdrop-filter` (no-op here) | CSS |
//! | Viewport pin | `WKWebView` in the content view | `WebKitWebView` in a `GtkFixed` | webview fills the window |
//! | Live resize | AppKit notifications | size-allocate + button mask | none |
//! | Lifecycle | accessory app, hide-on-blur in release | taskbar window, shown on launch | default window |

use tauri::{AppHandle, Manager, WebviewWindow, Window, WindowEvent};

use crate::host::PanelSize;
use crate::settings::Settings;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod other;

/// The OS-shaped half of the host. Shared code calls these methods; each
/// platform crate folder fills them in.
pub trait Host: Send + Sync {
    /// Process-wide toolkit prep. Runs before Tauri (and GTK) start.
    fn prepare(&self) {}

    /// App-level policy — Dock accessory, activation, anything that is not a
    /// window yet.
    fn configure_app(&self, _app: &AppHandle) {}

    /// Native frost / clear. No-op on hosts where the shell paints frost in CSS.
    fn set_frosted(&self, _window: &WebviewWindow, _frosted: bool) -> Result<(), String> {
        Ok(())
    }

    /// The window's size in CSS pixels. The page cannot measure this itself
    /// once the webview is larger than the window and pinned to a corner.
    fn panel_size(&self, window: &WebviewWindow) -> Result<PanelSize, String>;

    /// Size the webview to the stage and pin it so a resize does not jitter
    /// the composer.
    fn fit_viewport(&self, window: &WebviewWindow) -> Result<(), String>;

    /// Keep [`fit_viewport`] honest as the window is dragged, and tell the
    /// page the size on every step.
    fn watch_viewport(&self, window: &WebviewWindow) -> Result<(), String>;

    /// Tell the page when a live resize drag starts and ends, so the border
    /// glow can stay lit while the OS has the pointer.
    fn watch_live_resize(&self, _window: &WebviewWindow) -> Result<(), String> {
        Ok(())
    }

    /// After the shared attach (size, frost, watches). Linux uses this to put
    /// the panel on the taskbar and show it; macOS leaves it hidden until the
    /// tray summons it.
    fn after_attach(&self, _window: &WebviewWindow, _settings: &Settings) {}

    /// Drop leftover compositor tiles after the page moves a bubble. No-op
    /// where the webview already clears vacated pixels.
    fn flush_compositor(&self, _window: &WebviewWindow) -> Result<(), String> {
        Ok(())
    }

    /// OS-specific window events. Shared close-to-hide lives in `main`.
    fn on_window_event(&self, _window: &Window, _event: &WindowEvent) {}
}

/// The host for this binary. Compile-time DI: only one of the platform
/// modules is in the crate graph.
pub fn current() -> &'static dyn Host {
    #[cfg(target_os = "macos")]
    {
        &macos::Macos
    }
    #[cfg(target_os = "linux")]
    {
        &linux::Linux
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        &other::Other
    }
}

/// Frost, pin, and watch the panel window through the injected host.
pub fn bind_window(window: &WebviewWindow, settings: &Settings) {
    let host = current();
    if let Err(error) = host.set_frosted(window, true) {
        eprintln!("[nessa] could not frost the panel: {error}");
    }
    if let Err(error) = host.fit_viewport(window) {
        eprintln!("[nessa] could not fit the panel's viewport: {error}");
    }
    if let Err(error) = host.watch_viewport(window) {
        eprintln!("[nessa] could not watch the panel's viewport: {error}");
    }
    if let Err(error) = host.watch_live_resize(window) {
        eprintln!("[nessa] could not watch live resize: {error}");
    }
    host.after_attach(window, settings);
}

/// Lets the frontend follow its own surface toggle: the frosted surface is a
/// window-level effect on macOS, so the clear surface has to turn it off
/// natively as well as in CSS. The tray's check mark is reflected from the
/// same call, which keeps the frontend the single source of truth.
#[tauri::command]
pub fn set_frosted(
    app: AppHandle,
    window: WebviewWindow,
    frosted: bool,
) -> Result<(), String> {
    current().set_frosted(&window, frosted)?;

    if let Some(item) = app.try_state::<crate::tray::SurfaceMenuItem>() {
        let _ = item.0.set_checked(!frosted);
    }

    Ok(())
}

/// The window's size now, for a page that has just loaded and has no size
/// event coming — a devtools reload mid-session, or a webview that mounted
/// after the panel was last fitted.
#[tauri::command]
pub fn panel_size(window: WebviewWindow) -> Result<PanelSize, String> {
    current().panel_size(&window)
}

/// Clears leftover compositor tiles in the webview. Linux needs this when
/// the transcript slides; other hosts already paint vacated pixels.
#[tauri::command]
pub fn flush_compositor(window: WebviewWindow) -> Result<(), String> {
    current().flush_compositor(&window)
}

/// The display the panel is on, falling back to the primary one — the same
/// choice `panel::anchor_to_edge` makes when it places the panel.
pub(crate) fn current_monitor(
    window: &WebviewWindow,
) -> Result<Option<tauri::Monitor>, String> {
    let current = window.current_monitor().map_err(|e| e.to_string())?;
    match current {
        Some(monitor) => Ok(Some(monitor)),
        None => window.primary_monitor().map_err(|e| e.to_string()),
    }
}
