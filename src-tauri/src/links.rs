//! Where a clicked link goes.
//!
//! Nessa's window is a floating bar with no chrome: no address bar, no back
//! button, no tab to close. A webview that navigates away from `index.html` has
//! no way back, so the bar simply becomes whatever page was clicked until the
//! app is restarted.
//!
//! It is also the wrong place for a page to land. The panel's webview is the
//! surface the host's commands are granted to (`capabilities/default.json`
//! names the `main` and `setup` windows), and the links in it come from model
//! output and from whatever a tool fetched. Content that arrived over the wire
//! should not end up inside the window that can ask the host for things.
//!
//! So this refuses every navigation out of the app, and hands the ones a person
//! plainly meant — `http`, `https`, `mailto` — to the OS to open in whatever
//! browser or mail client they actually use. Anything else is refused without
//! being handed anywhere: `file:`, `data:`, `javascript:` and unknown custom
//! schemes are all ways to make the OS act on text the agent produced.
//!
//! ```text
//! click -> decide(url) -> Allow          -> the webview navigates (our own page)
//!                      -> HandToBrowser  -> Host::open_externally, navigation cancelled
//!                      -> Refuse         -> nothing happens, one line on stderr
//! ```
//!
//! An arrow that stops is not a dead end: cancelling a navigation leaves the
//! webview exactly where it was, on the page it already has, so a refusal costs
//! a click rather than the window.
//!
//! The decision is a pure function over the URL so it can be tested without a
//! webview; the plugin is only the wiring that applies it to every window,
//! including the one `tauri.conf.json` declares rather than code builds.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime, Url,
};

use crate::platform;

/// What should happen to a navigation the webview is about to make.
#[derive(Debug, PartialEq, Eq)]
pub enum Navigation {
    /// Our own page. The webview keeps it.
    Allow,
    /// Somewhere a person meant to go, but not in here.
    HandToBrowser,
    /// Nothing a click in a panel should be able to reach.
    Refuse,
}

/// Whether this host is the app talking to itself rather than the web.
///
/// `localhost` and `127.0.0.1` are the dev server (`tauri.conf.json` sets
/// `devUrl` to `http://localhost:1420`) and the local gateway the panel talks
/// to. Everything under `.localhost` is a Tauri custom protocol as it appears
/// where those are served over `http` — `tauri.localhost` for the app's own
/// pages, `ipc.localhost` for the command bridge, `asset.localhost` for local
/// files — and the suffix covers the ones a future Tauri adds too.
///
/// The suffix is a full label, so a host that merely *contains* one of these
/// names is not one of ours: `localhost.evil.com` ends in `.com`, and
/// `evil-localhost` is its own single label.
fn own_host(host: Option<&str>) -> bool {
    match host {
        Some(host) => host == "localhost" || host == "127.0.0.1" || host.ends_with(".localhost"),
        None => false,
    }
}

/// Where this URL belongs. See the module documentation for the reasoning.
pub fn decide(url: &Url) -> Navigation {
    match url.scheme() {
        // The custom protocols the app is served over, where the platform
        // gives them a scheme of their own rather than an http host.
        "tauri" | "ipc" | "asset" => Navigation::Allow,
        "http" | "https" if own_host(url.host_str()) => Navigation::Allow,
        "http" | "https" | "mailto" => Navigation::HandToBrowser,
        _ => Navigation::Refuse,
    }
}

/// Applies [`decide`] to every window in the app.
///
/// A plugin rather than `WebviewWindowBuilder::on_navigation` because the panel
/// window is declared in `tauri.conf.json`, so no builder call in this crate
/// ever sees it. The plugin hook runs for every webview however it was made,
/// which is also what keeps a window added later from quietly missing this.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("nessa-links")
        .on_navigation(|_webview, url| match decide(url) {
            Navigation::Allow => true,
            Navigation::HandToBrowser => {
                if let Err(error) = platform::current().open_externally(url.as_str()) {
                    eprintln!("[nessa] could not open {url} in the browser: {error}");
                }
                false
            }
            Navigation::Refuse => {
                eprintln!("[nessa] refused to open {url} from a Nessa window");
                false
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::{decide, Navigation};
    use tauri::Url;

    fn decision(url: &str) -> Navigation {
        decide(&Url::parse(url).expect("a URL the webview could navigate to"))
    }

    #[test]
    fn the_app_s_own_pages_are_kept_in_the_window() {
        for url in [
            "tauri://localhost/index.html",
            "tauri://localhost/index.html?surface=setup",
            "http://tauri.localhost/index.html",
            "https://tauri.localhost/index.html?surface=setup",
            "http://ipc.localhost/",
            "asset://localhost/icon.png",
            "http://asset.localhost/Users/someone/Pictures/shot.png",
            // The dev server `tauri.conf.json` points `devUrl` at, and the
            // local gateway the panel talks to.
            "http://localhost:1420/index.html",
            "http://127.0.0.1:7420/health",
        ] {
            assert_eq!(decision(url), Navigation::Allow, "{url}");
        }
    }

    #[test]
    fn the_web_is_handed_to_the_browser_rather_than_shown_in_the_bar() {
        for url in [
            "https://anthropic.com/",
            "http://example.com/page?q=1#x",
            "mailto:someone@example.com",
        ] {
            assert_eq!(decision(url), Navigation::HandToBrowser, "{url}");
        }
    }

    /// Every one of these is a way to make the OS act on text a model wrote, so
    /// none of them is handed anywhere — refusing is the whole point.
    #[test]
    fn nothing_else_is_opened_at_all() {
        for url in [
            "file:///etc/passwd",
            "file:///Applications/Calculator.app",
            "data:text/html,<script>fetch('http://evil')</script>",
            "javascript:fetch('http://evil')",
            "vscode://file/Users/someone/.ssh/id_rsa",
            "smb://192.168.1.1/share",
            "ftp://example.com/",
            // Our own protocols are ours only as schemes; the same names as a
            // *host* under someone else's scheme are not.
            "ssh://tauri.localhost/",
        ] {
            assert_eq!(decision(url), Navigation::Refuse, "{url}");
        }
    }

    /// A host that merely looks like one of ours is not one of ours: these all
    /// resolve on the public internet, so they go out to the browser like any
    /// other web address, and are never treated as the app's own page.
    #[test]
    fn a_lookalike_host_is_not_this_app() {
        for url in [
            "http://evil-localhost/",
            "https://localhost.evil.com/",
            "http://ipc.localhost.evil.com/",
            "http://127.0.0.1.evil.com/",
            "https://notlocalhost/",
            "http://127.0.0.10/",
        ] {
            assert_eq!(decision(url), Navigation::HandToBrowser, "{url}");
        }
    }
}
