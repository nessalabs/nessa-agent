//! Where a clicked link goes.
//!
//! Nessa's window is a floating bar with no chrome: no address bar, no back
//! button, no tab to close. A webview that navigates away from `index.html` has
//! no way back, so the bar simply becomes whatever page was clicked until the
//! app is restarted. Hiding and showing the panel does not reload it.
//!
//! It is also the wrong place for a page to land. The panel's webview is the
//! surface the host's commands are granted to (`capabilities/default.json`
//! names the `main` and `setup` windows), and the links in it come from model
//! output and from whatever a tool fetched.
//!
//! So this allows exactly the origins the app serves its own pages on, hands
//! the links a person plainly meant — `http`, `https`, `mailto` — to the OS to
//! open in whatever browser or mail client they actually use, and refuses
//! everything else without handing it anywhere: `file:`, `data:`, `javascript:`
//! and unknown custom schemes are all ways to make the OS act on text the agent
//! produced.
//!
//! ```text
//! click -> decide(url) -> Allow          -> the webview navigates (our own page)
//!                      -> HandToBrowser  -> Host::open_externally, navigation cancelled
//!                      -> Refuse         -> cancelled, and the person is told why
//! ```
//!
//! An arrow that stops is not a dead end: cancelling a navigation leaves the
//! webview exactly where it was, on the page it already has, so a refusal costs
//! a click rather than the window.
//!
//! # The allow-list is the app's own origins, and nothing wider
//!
//! Loopback in general is *not* this app. The panel's own traffic to the local
//! gateway is `fetch` and a WebSocket, which never reach a navigation policy,
//! so allowing `127.0.0.1` bought nothing — while a model that writes
//! `[report](http://127.0.0.1:7420/session)` would have stranded the bar on it,
//! which is the bug this module exists to close. The origins below are the only
//! ones the app is ever served on:
//!
//! | Origin | Where |
//! |---|---|
//! | `tauri://localhost` | macOS, and any host serving the app over the custom protocol |
//! | `http://tauri.localhost` | Linux and Windows, where that protocol is an http host |
//! | `http://localhost:1420` | the dev server alone, and only in a `tauri dev` build |
//!
//! # Which ways out of the page this actually sees
//!
//! The navigation policy is not every exit. Covered on both macOS and Linux:
//! `<a href>`, `location =`, `<meta refresh>`, form submits, redirect chains
//! and iframe loads. Not covered: `window.open(...)` on either host,
//! `<a target="_blank">` on Linux (wry routes it to a new-window handler Tauri
//! does not set, so it is dropped rather than opened), and a download-flagged
//! navigation on macOS — `<a download>` or an option-click — which WebKit takes
//! before the policy handler runs.
//!
//! Those are shut elsewhere rather than here, and are recorded so the next
//! person does not read this module as a complete gate: the CSP sets
//! `frame-src 'none'` and `form-action 'none'`, and the transcript renders
//! markdown with no raw HTML, so model output cannot author a `target`, a
//! `download` or a script that calls `window.open`. A change to either of those
//! re-opens a route past this file.
//!
//! A dropped URL is the other way a page can be made to navigate, and the page
//! stops that itself in `src/panel/adapters/use-drop-navigation-guard.ts`. One
//! that got past it would arrive here and be refused or handed out like any
//! other.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Emitter, Runtime, Url, Webview,
};

use crate::host::{LinkNotOpened, LINK_NOT_OPENED};
use crate::platform::{self, Host};

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
fn serving() -> Serving {
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

/// Where this URL belongs. See the module documentation for the reasoning.
///
/// A `mailto:` is handed over whole, query and all. Some mail clients have
/// historically honoured an `?attach=` in one; macOS Mail does not, and the
/// alternative — rewriting a person's link before their mail client sees it —
/// would break the ordinary `?subject=` this is mostly used for.
pub fn decide(url: &Url, serving: Serving) -> Navigation {
    if own_page(url, serving) {
        return Navigation::Allow;
    }
    match url.scheme() {
        "http" | "https" | "mailto" => Navigation::HandToBrowser,
        _ => Navigation::Refuse,
    }
}

/// What one navigation leaves behind: whether the webview keeps it, and what
/// the person is owed when it does not.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// The webview navigates. Our own page.
    Navigate,
    /// Cancelled, and nothing needs saying: it went out to the browser.
    Handed,
    /// Cancelled, and the click would otherwise have done nothing visible.
    NotOpened(LinkNotOpened),
}

/// The policy and its effect, with the host injected so both can be tested
/// together without a webview. [`init`] supplies the real one.
pub fn apply(host: &dyn Host, url: &Url, serving: Serving) -> Applied {
    match decide(url, serving) {
        Navigation::Allow => Applied::Navigate,
        Navigation::HandToBrowser => match host.open_externally(url.as_str()) {
            Ok(()) => Applied::Handed,
            // The browser is the person's, not ours: when we cannot reach it
            // there is nothing left to try, only something to say.
            Err(error) => Applied::NotOpened(LinkNotOpened::failed(url.as_str(), &error)),
        },
        Navigation::Refuse => Applied::NotOpened(LinkNotOpened::refused(url.as_str())),
    }
}

/// Tells the page a click did nothing, and why.
///
/// A packaged macOS app's stderr reaches nobody, and the release Windows build
/// has no console at all, so stderr alone is a click that silently does
/// nothing. The line is still written: it is where a bug report finds the URL.
fn report<R: Runtime>(webview: &Webview<R>, notice: LinkNotOpened) {
    eprintln!("[nessa] {}: {}", notice.reason.as_sentence(), notice.url);
    if let Err(error) = webview.emit(LINK_NOT_OPENED, notice) {
        eprintln!("[nessa] could not tell the panel about that link: {error}");
    }
}

/// Applies [`apply`] to every window in the app.
///
/// A plugin rather than `WebviewWindowBuilder::on_navigation` because the panel
/// window is declared in `tauri.conf.json`, so no builder call in this crate
/// ever sees it. The plugin hook runs for every webview however it was made,
/// which is also what keeps a window added later from quietly missing this.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("nessa-links")
        .on_navigation(
            |webview, url| match apply(platform::current(), url, serving()) {
                Applied::Navigate => true,
                Applied::Handed => false,
                Applied::NotOpened(notice) => {
                    report(webview, notice);
                    false
                }
            },
        )
        .build()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tauri::{Url, WebviewWindow};

    use super::{apply, decide, Applied, Navigation, Serving};
    use crate::host::{LinkNotOpened, NotOpened, PanelSize};
    use crate::platform::Host;

    fn decision(url: &str) -> Navigation {
        packaged(url)
    }

    fn packaged(url: &str) -> Navigation {
        decide(&parse(url), Serving::Packaged)
    }

    fn in_dev(url: &str) -> Navigation {
        decide(&parse(url), Serving::DevServer)
    }

    fn parse(url: &str) -> Url {
        Url::parse(url).expect("a URL the webview could navigate to")
    }

    /// A host that only writes down what it was asked to open, so a test can
    /// say both what reached the OS and what did not.
    #[derive(Default)]
    struct Opened {
        urls: Mutex<Vec<String>>,
        refuse: Option<String>,
    }

    impl Opened {
        fn refusing(reason: &str) -> Self {
            Self {
                refuse: Some(reason.into()),
                ..Self::default()
            }
        }

        fn urls(&self) -> Vec<String> {
            self.urls
                .lock()
                .expect("no test panicked holding it")
                .clone()
        }
    }

    impl Host for Opened {
        fn panel_size(&self, _window: &WebviewWindow) -> Result<PanelSize, String> {
            unreachable!("a link decision never measures the panel")
        }

        fn fit_viewport(&self, _window: &WebviewWindow) -> Result<(), String> {
            unreachable!("a link decision never fits the viewport")
        }

        fn watch_viewport(&self, _window: &WebviewWindow) -> Result<(), String> {
            unreachable!("a link decision never watches the viewport")
        }

        fn open_externally(&self, url: &str) -> Result<(), String> {
            self.urls
                .lock()
                .expect("no test panicked holding it")
                .push(url.into());
            match &self.refuse {
                Some(reason) => Err(reason.clone()),
                None => Ok(()),
            }
        }
    }

    fn applied(host: &Opened, url: &str) -> Applied {
        apply(host, &parse(url), Serving::Packaged)
    }

    #[test]
    fn the_app_s_own_pages_are_kept_in_the_window() {
        for url in [
            "tauri://localhost/index.html",
            "tauri://localhost/index.html?surface=setup",
            "http://tauri.localhost/index.html",
            "http://tauri.localhost/index.html?surface=setup",
        ] {
            assert_eq!(decision(url), Navigation::Allow, "{url}");
        }
    }

    /// The dev server is an app origin while `tauri dev` serves the page, and
    /// somebody else's web address in the app people install.
    #[test]
    fn the_dev_server_is_the_app_only_in_a_dev_build() {
        for url in [
            "http://localhost:1420/index.html",
            "http://127.0.0.1:1420/",
            "http://[::1]:1420/",
        ] {
            assert_eq!(in_dev(url), Navigation::Allow, "{url}");
            assert_eq!(packaged(url), Navigation::HandToBrowser, "{url}");
        }
    }

    /// The whole point of the narrowing: loopback is not this app. A link a
    /// model wrote to the local gateway, or to anything else listening on this
    /// machine, would strand the bar on it exactly as an outside page would.
    #[test]
    fn the_rest_of_loopback_is_not_this_app_even_in_dev() {
        for url in [
            // The gateway the panel itself talks to — over fetch and a
            // WebSocket, neither of which is a navigation.
            "http://127.0.0.1:7420/session",
            "http://localhost:7420/health",
            "http://localhost:8888/",
            "http://localhost:22/",
            // Other spellings of 127.0.0.1 that the URL parser normalises.
            "http://127.1/",
            "http://0x7f000001/",
        ] {
            assert_eq!(in_dev(url), Navigation::HandToBrowser, "{url}");
            assert_eq!(packaged(url), Navigation::HandToBrowser, "{url}");
        }
    }

    /// `*.localhost` resolves to loopback on Linux and fails DNS on macOS. Both
    /// are a stranded bar, and neither is an origin this app is served on.
    #[test]
    fn only_the_protocol_host_itself_is_the_app_under_localhost() {
        for url in [
            "http://evil.localhost:7420/anything",
            "http://evil.localhost/",
            "http://.localhost/",
            "http://ipc.localhost/",
            "http://asset.localhost/Users/someone/Pictures/shot.png",
            // A real protocol host, but on a port it is never served on.
            "http://tauri.localhost:7420/",
            "https://tauri.localhost/",
        ] {
            assert_eq!(in_dev(url), Navigation::HandToBrowser, "{url}");
        }
    }

    #[test]
    fn the_web_is_handed_to_the_browser_rather_than_shown_in_the_bar() {
        for url in [
            "https://anthropic.com/",
            "http://example.com/page?q=1#x",
            "mailto:someone@example.com",
            "mailto:someone@example.com?subject=Nessa",
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
            // Our own protocols are ours as schemes, not as hostnames under
            // somebody else's scheme.
            "ssh://tauri.localhost/",
            "asset://localhost/icon.png",
            "ipc://localhost/",
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
            "http://tauri.localhost.evil.com/",
            "http://127.0.0.1.evil.com/",
            "https://notlocalhost/",
            "http://127.0.0.10/",
        ] {
            assert_eq!(decision(url), Navigation::HandToBrowser, "{url}");
        }
    }

    #[test]
    fn a_web_link_reaches_the_opener_byte_for_byte() {
        let host = Opened::default();
        assert_eq!(
            applied(&host, "https://anthropic.com/a%20b?q=1#x"),
            Applied::Handed
        );
        assert_eq!(host.urls(), ["https://anthropic.com/a%20b?q=1#x"]);
    }

    /// The assertion that keeps the opener from becoming a way to reach the
    /// machine: a refused scheme is not merely not navigated to, it is not
    /// handed to the OS either.
    #[test]
    fn a_refused_link_never_reaches_the_opener() {
        let host = Opened::default();
        for url in [
            "file:///etc/passwd",
            "javascript:fetch('http://evil')",
            "vscode://x",
        ] {
            assert_eq!(
                applied(&host, url),
                Applied::NotOpened(LinkNotOpened::refused(url)),
                "{url}"
            );
        }
        assert_eq!(host.urls(), Vec::<String>::new());
    }

    #[test]
    fn our_own_page_is_navigated_to_and_opens_nothing() {
        let host = Opened::default();
        assert_eq!(
            applied(&host, "tauri://localhost/index.html"),
            Applied::Navigate
        );
        assert_eq!(host.urls(), Vec::<String>::new());
    }

    /// A host that cannot reach a browser owes the person the reason rather
    /// than a click that does nothing — the Windows default is exactly this.
    #[test]
    fn an_opener_that_fails_is_reported_rather_than_swallowed() {
        let host = Opened::refusing("no xdg-open on PATH");
        let applied = applied(&host, "https://anthropic.com/");
        assert_eq!(host.urls(), ["https://anthropic.com/"]);
        match applied {
            Applied::NotOpened(notice) => {
                assert_eq!(notice.reason, NotOpened::OpenerFailed);
                assert_eq!(notice.url, "https://anthropic.com/");
                assert_eq!(notice.detail.as_deref(), Some("no xdg-open on PATH"));
            }
            other => panic!("a failed opener should be reported, got {other:?}"),
        }
    }
}
