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

/// A link the person clicked did not open, and nothing on screen changed. The
/// payload is [`LinkNotOpened`]. Sent to the window the click happened in: a
/// click that does nothing needs a sentence, and on a packaged app stderr is
/// not one — macOS sends it nowhere a person looks, and the release Windows
/// build has no console at all.
pub const LINK_NOT_OPENED: &str = "nessa://link-not-opened";
/// The managed gateway's startup projection changed. The payload is
/// [`GatewayStartup`]; a snapshot command carries the same revisioned value so
/// subscribing before asking cannot lose or reorder a transition.
pub const GATEWAY_STARTUP: &str = "nessa://gateway-startup";

/// Whether the host could put itself together at launch (ADR 221). Asked by
/// the page before anything else; `Refused` means no other command that needs
/// the host's dependencies will answer, and the page shows only the refusal.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum HostStartup {
    Ready,
    /// `details` is the technical reason, for whoever helps the person. The
    /// page shows a plain sentence and keeps this behind "Details".
    Refused {
        details: String,
    },
}

/// What the desktop host currently knows about gateway startup.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum GatewayStartup {
    /// This build does not manage a packaged gateway. The surface keeps using
    /// its direct readiness probe and draws no conclusion from the host.
    Unmanaged { revision: u64 },
    /// The host is reconciling its registered service; `step` says where.
    Starting { revision: u64, step: StartupStep },
    /// The exact registered PID, generation, instance and fingerprint agree.
    Ready { revision: u64 },
    /// Reconciliation stopped. `message` names the safe next action when known.
    Failed { revision: u64, message: String },
}

/// Where a starting gateway is. The page turns each into a plain sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupStep {
    Preparing,
    Replacing,
    Launching,
}

/// Why a clicked link did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotOpened {
    /// The scheme is not one a link in a panel may reach — `file:`, `data:`,
    /// `javascript:`, an editor's custom scheme. Handing it to the OS is the
    /// risk, so it was handed nowhere.
    Refused,
    /// It was a web or mail address, and the browser or mail client could not
    /// be started.
    OpenerFailed,
}

impl NotOpened {
    /// The host's own half-line for its diagnostics. The sentence a person
    /// reads is the panel's, in `src/panel/application/link-notice.ts`.
    pub fn as_sentence(self) -> &'static str {
        match self {
            Self::Refused => "refused to open",
            Self::OpenerFailed => "could not open",
        }
    }
}

/// Which link, and why it did nothing. The URL is carried so the panel can
/// show the person what they clicked; `detail` is the opener's own error, which
/// is technical and belongs with the diagnostics rather than on screen.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LinkNotOpened {
    pub url: String,
    pub reason: NotOpened,
    pub detail: Option<String>,
}

impl LinkNotOpened {
    /// A scheme a panel may not reach. There is no opener error to carry:
    /// nothing was attempted.
    pub fn refused(url: &str) -> Self {
        Self {
            url: url.into(),
            reason: NotOpened::Refused,
            detail: None,
        }
    }

    /// The browser or mail client could not be started, and why.
    pub fn failed(url: &str, detail: &str) -> Self {
        Self {
            url: url.into(),
            reason: NotOpened::OpenerFailed,
            detail: Some(detail.into()),
        }
    }
}

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

    #[test]
    fn shell_gateway_startup_matches_the_host() {
        let shell = include_str!("../../src/startup/application/ports.ts");
        let at = shell
            .find("export type GatewayStartup")
            .expect("the shell is missing GatewayStartup");
        let declaration = &shell[at..];
        let end = declaration
            .find("export interface GatewayStartupSource")
            .expect("GatewayStartup declaration closes before its source");
        let declaration = &declaration[..end];

        assert!(declaration.contains("revision: number"));
        for state in ["unmanaged", "starting", "ready", "failed"] {
            assert!(
                declaration.contains(&format!("state: \"{state}\"")),
                "GatewayStartup is missing {state}"
            );
        }
        assert!(declaration.contains("message: string"));
        assert!(declaration.contains("step: StartupStep"));
        let at = shell
            .find("export type StartupStep")
            .expect("the shell is missing StartupStep");
        let steps = &shell[at..shell[at..].find('\n').map(|end| at + end).unwrap()];
        for step in ["preparing", "replacing", "launching"] {
            assert!(
                steps.contains(&format!("\"{step}\"")),
                "StartupStep is missing {step}"
            );
        }
    }

    #[test]
    fn shell_host_startup_matches_the_host() {
        let shell = include_str!("../../src/startup/application/ports.ts");
        let at = shell
            .find("export type HostStartup")
            .expect("the shell is missing HostStartup");
        let declaration = &shell[at..];
        for piece in ["state: \"ready\"", "state: \"refused\"", "details: string"] {
            assert!(
                declaration.contains(piece),
                "HostStartup is missing {piece}"
            );
        }
    }
}
