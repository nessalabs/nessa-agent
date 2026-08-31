//! The host/shell seam.
//!
//! Every name and payload that crosses into the webview is declared here. The
//! other side is `src/host-window.ts`. The test at the bottom fails if a name
//! here is not listed there — two declarations, one check, so they cannot drift
//! silently. Generating one side from the other is the next step if this list
//! grows past a handful of events.

/// Raised when the tray item is chosen; the frontend flips its own surface
/// state and calls back through `set_frosted`.
pub const TOGGLE_SURFACE: &str = "nessa://toggle-surface";
/// Raised whenever the panel is summoned, so the composer takes the caret
/// without the reader having to click into it first.
pub const FOCUS_COMPOSER: &str = "nessa://focus-composer";
/// Carries the window's size to the page, which can no longer measure it
/// once the webview is detached from the window (see `viewport`).
pub const PANEL_SIZED: &str = "nessa://panel-sized";
/// Emitted as the user takes hold of the window's frame.
pub const RESIZE_STARTED: &str = "nessa://resize-started";
/// Emitted as they let go of it.
pub const RESIZE_ENDED: &str = "nessa://resize-ended";

/// Points, which are CSS pixels: the webview does its own scaling, so no device
/// ratio enters into it.
#[derive(Clone, serde::Serialize)]
pub struct PanelSize {
    pub width: f64,
    pub height: f64,
}

impl PanelSize {
    pub fn from_logical(width: f64, height: f64) -> Option<Self> {
        if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
            Some(Self { width, height })
        } else {
            None
        }
    }
}

/// The window's inner size in CSS pixels. Used on hosts where the webview
/// still fills the window (everywhere the viewport is not detached).
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn logical_inner_size(window: &tauri::WebviewWindow) -> Result<PanelSize, String> {
    let size = window.inner_size().map_err(|error| error.to_string())?;
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    PanelSize::from_logical(
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    )
    .ok_or_else(|| String::from("the window has no size"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_listens_for_every_host_event() {
        let shell = include_str!("../../src/host-window.ts");
        for event in [
            TOGGLE_SURFACE,
            FOCUS_COMPOSER,
            PANEL_SIZED,
            RESIZE_STARTED,
            RESIZE_ENDED,
        ] {
            assert!(
                shell.contains(&format!("\"{event}\"")),
                "host-window.ts does not list {event}"
            );
        }
    }
}
