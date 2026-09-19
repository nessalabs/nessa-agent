//! The host/shell seam.
//!
//! Every name and payload that crosses into the webview is declared here. The
//! other side is `src/host/window.ts`. The test at the bottom fails if a name
//! here is not listed there — two declarations, one check, so they cannot drift
//! silently. Generating one side from the other is the next step if this list
//! grows past a handful of events.

/// Raised when the tray item is chosen; the frontend flips its own surface
/// state and calls back through `set_frosted`.
pub const TOGGLE_SURFACE: &str = "nessa://toggle-surface";
/// Raised whenever the panel is summoned, so the composer takes the caret
/// without the reader having to click into it first.
pub const FOCUS_COMPOSER: &str = "nessa://focus-composer";
/// The global summon accelerator fired; the payload is whether the panel is
/// now showing. The accelerator is registered with the system, so the key press
/// never reaches a window as a key — a surface that needs to know it happened
/// has to be told.
pub const SUMMONED: &str = "nessa://summoned";
/// Carries the window's size to the page, which can no longer measure it
/// once the webview is detached from the window (see `platform`).
pub const PANEL_SIZED: &str = "nessa://panel-sized";
/// Emitted as the user takes hold of the window's frame. Only the hosts with
/// live-resize notifications raise it; the seam still declares every protocol
/// name on every target so the shell listing and its drift test stay complete.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub const RESIZE_STARTED: &str = "nessa://resize-started";
/// Emitted as they let go of it. Raised by the same hosts as
/// [`RESIZE_STARTED`].
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub const RESIZE_ENDED: &str = "nessa://resize-ended";
/// A check found a newer published release; the payload is
/// [`crate::updater::Release`]. Sent to the panel alone — setup has nowhere to
/// put it — and kept on the host as well, so a panel that was closed or had not
/// loaded yet can ask for the same value on mount.
pub const UPDATE_AVAILABLE: &str = "nessa://update-available";
/// How far the download the panel asked for has got; the payload is
/// [`crate::updater::Downloaded`]. Throttled to one event per position of the
/// bar rather than one per chunk.
pub const UPDATE_PROGRESS: &str = "nessa://update-progress";
/// The install the panel asked for did not happen; the payload is the reason.
/// The panel turns it into a plain statement and a retry — this is the one
/// update failure that reaches the screen, because it is the one somebody
/// asked for.
pub const UPDATE_FAILED: &str = "nessa://update-failed";

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

#[cfg(test)]
mod tests {
    // No `use super::*`: the constants are read out of the source rather than
    // named, which is the point.

    /// Every `nessa://` name this file declares, read out of the file itself.
    ///
    /// Listing them by hand made the drift this test exists to prevent possible
    /// in the test: a tenth `pub const` and a forgotten tenth entry, and the
    /// check goes on passing while the shell never hears the new event. Read
    /// from the source, adding the constant is what adds it here.
    fn declared_events(source: &str) -> Vec<&str> {
        source
            .lines()
            // The declarations, and only those: scanning the whole file for the
            // prefix also finds this scraper's own search string.
            .filter(|line| line.starts_with("pub const"))
            .filter_map(|line| {
                let start = line.find("\"nessa://")? + 1;
                let literal = &line[start..];
                Some(&literal[..literal.find('"')?])
            })
            .collect()
    }

    #[test]
    fn shell_listens_for_every_host_event() {
        let shell = include_str!("../../src/host/window.ts");
        let events = declared_events(include_str!("host.rs"));

        // A scrape that found nothing would pass every assertion below.
        assert!(
            events.len() >= 9,
            "only {} event names were found in host.rs; the scrape is broken",
            events.len()
        );
        for event in events {
            assert!(
                shell.contains(&format!("\"{event}\"")),
                "src/host/window.ts does not list {event}"
            );
        }
    }

    /// The two sides of one payload, and the fields are looked for inside the
    /// interface rather than anywhere in the file — `width: number` appears in
    /// any number of unrelated shapes, so the old check passed on a
    /// `PanelSize` that had lost a field.
    #[test]
    fn shell_panel_size_matches_the_host() {
        let shell = include_str!("../../src/host/window.ts");
        let at = shell
            .find("export interface PanelSize")
            .expect("src/host/window.ts is missing PanelSize");
        let body = &shell[at..];
        let end = body.find('}').expect("PanelSize closes");
        let declared = &body[..end];

        for field in ["width", "height"] {
            assert!(
                declared.contains(&format!("{field}: number")),
                "src/host/window.ts PanelSize is missing {field}"
            );
        }
    }
}
