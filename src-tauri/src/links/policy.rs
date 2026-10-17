//! Navigation decisions. The parent owns the opener and webview plugin.

use tauri::Url;

use crate::app_sandbox;

/// The port `tauri.conf.json` gives `devUrl`. The dev server is a real origin
/// the app is served on, but only while `tauri dev` is driving it, so it is an
/// exception with a build behind it rather than a hole in the packaged app.
const DEV_SERVER_PORT: u16 = 1420;

/// Which build is asking, because the dev server is an app origin in one of
/// them and someone else's web address in the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serving {
    /// A packaged app. Its pages come from the custom protocol, full stop.
    Packaged,
    /// `tauri dev`, where the page is served by Vite over loopback.
    DevServer,
}

/// Which build this binary is, from the `dev` cfg `tauri-build` sets.
pub(super) fn serving() -> Serving {
    if cfg!(dev) {
        Serving::DevServer
    } else {
        Serving::Packaged
    }
}

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

/// Whether this is the app's own page, served by the app itself.
///
/// Matched as a whole origin — scheme, host *and* port — so neither a
/// lookalike host (`localhost.evil.com`, `evil-localhost`) nor another port on
/// a real one (`http://tauri.localhost:7420/`) reads as the app.
fn own_page(url: &Url, serving: Serving) -> bool {
    match (url.scheme(), url.host_str()) {
        // The custom protocol where the platform gives it a scheme of its own.
        ("tauri", Some("localhost")) => true,
        // The same protocol where the platform serves it as an http host.
        ("http", Some("tauri.localhost")) => url.port().is_none(),
        // Vite, in a dev build only. `[::1]` is spelled with its brackets in a
        // parsed host, and is the same server as `localhost` on a machine that
        // resolves the name to IPv6.
        ("http", Some("localhost" | "127.0.0.1" | "[::1]")) => {
            serving == Serving::DevServer && url.port() == Some(DEV_SERVER_PORT)
        }
        _ => false,
    }
}

/// Whether this is the frame an MCP App is drawn in ([`app_sandbox`]): the
/// sandbox proxy on its own scheme — an `http` host on Windows — or the app's
/// document inside it, which the proxy loads as `about:srcdoc`.
///
/// The navigation policy cannot tell a frame's load from the window's, so a
/// link to the proxy clicked in the window would load it there too; the proxy
/// goes back at once when it finds itself the top page
/// (`src/desktop/widgets/app/sandbox/proxy.html`).
fn app_frame(url: &Url) -> bool {
    match (url.scheme(), url.host_str()) {
        (scheme, Some("localhost")) if scheme == app_sandbox::SCHEME => url.port().is_none(),
        ("http", Some(host)) if host == app_sandbox::HTTP_HOST => url.port().is_none(),
        ("about", None) => url.path() == "srcdoc",
        _ => false,
    }
}

/// Where this URL belongs. See the module documentation for the reasoning.
///
/// A `mailto:` is handed over whole, query and all. Some mail clients have
/// historically honoured an `?attach=` in one; macOS Mail does not, and the
/// alternative — rewriting a person's link before their mail client sees it —
/// would break the ordinary `?subject=` this is mostly used for.
pub fn decide(url: &Url, serving: Serving) -> Navigation {
    // The page a dev build shows when its own document never arrived. A
    // packaged build refuses it: the scheme is registered either way, and
    // only `tauri dev` may navigate there.
    if own_page(url, serving)
        || app_frame(url)
        || crate::page_load::is_load_failure_page(url, serving == Serving::DevServer)
    {
        return Navigation::Allow;
    }
    match url.scheme() {
        "http" | "https" | "mailto" => Navigation::HandToBrowser,
        _ => Navigation::Refuse,
    }
}
