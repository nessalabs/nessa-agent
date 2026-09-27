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
//! | Lifecycle | accessory app, stays open when focus moves away | taskbar window, shown on launch | default window |
//! | Clicked link | `/usr/bin/open` | `xdg-open` | refused, with a reason |

use tauri::{AppHandle, Manager, WebviewWindow, Window, WindowEvent};

use crate::host::PanelSize;
use crate::settings::Settings;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
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

    /// Shape a window into a screen-covering overlay: its level, its frame, and
    /// which Spaces it joins. Setup uses this — it dims what is behind it, and a
    /// dim that stops at the menu bar is not a dim.
    ///
    /// Placing is not showing. The setup window is placed while still hidden and
    /// revealed by its own page once that page has rendered, so this must not
    /// order the window in or activate the app; [`Host::reveal_overlay`] does
    /// that.
    ///
    /// Rendered, not painted: a window left hidden here is never drawn, so its
    /// page is served no animation frames and cannot wait for one.
    ///
    /// This is the *only* place the overlay is positioned, and the default below
    /// is why: a host that cannot cover the screen still needs its overlay put
    /// somewhere sensible, and the alternative — asking the window builder to
    /// centre it — is not a fallback but a competitor. A builder position is
    /// re-applied asynchronously on macOS, so it landed *after* this call and
    /// pulled the screen-sized window back to where the unplaced one would have
    /// gone. One owner, and no second request for the window system to honour
    /// later. See `panel::build_setup_window`.
    fn place_overlay(&self, window: &WebviewWindow) {
        // Losing the placement costs a window in the wrong corner, not setup, so
        // it is reported rather than fatal.
        if let Err(error) = window.center() {
            eprintln!("[nessa] could not centre setup: {error}");
        }
    }

    /// Bring an already-placed overlay to the front and give it focus.
    ///
    /// Focus is part of revealing rather than an extra: Nessa is an accessory
    /// app, so it is not active until something makes it active, and an inactive
    /// app's window spends the first click on being activated. Setup is a window
    /// full of buttons, and every one of them would need pressing twice.
    fn reveal_overlay(&self, _window: &WebviewWindow) {}

    /// Stack the panel over, or back under, a screen-covering overlay. Setup
    /// covers everything the panel normally floats above, so the shortcut it
    /// teaches would otherwise summon the panel out of sight behind it.
    fn set_above_overlay(&self, _window: &WebviewWindow, _above: bool) {}

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

    /// Hand a link to whatever the person has set as their browser or mail
    /// client. Called only for URLs [`crate::links::decide`] has already
    /// allowed out — this is the effect, not the policy, and it must not be
    /// given a URL that policy has not looked at.
    ///
    /// The default is a refusal rather than a no-op: a host with no way to
    /// reach a browser should say so on the one line it costs, not swallow
    /// every link a person clicks.
    fn open_externally(&self, _url: &str) -> Result<(), String> {
        Err("this host cannot open a browser".into())
    }
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
pub fn set_frosted(app: AppHandle, window: WebviewWindow, frosted: bool) -> Result<(), String> {
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
/// choice `panel::anchor_to_edge` makes when it places the panel. Only the
/// hosts that pin the webview themselves need it; `platform/other` lets the
/// webview fill the window.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) fn current_monitor(window: &WebviewWindow) -> Result<Option<tauri::Monitor>, String> {
    let current = window.current_monitor().map_err(|e| e.to_string())?;
    match current {
        Some(monitor) => Ok(Some(monitor)),
        None => window.primary_monitor().map_err(|e| e.to_string()),
    }
}
