//! Page-load facts published by the host callback, shared with the deadline.
//!
//! No native URL accessor is used here: WKWebView may have no URL at all
//! before its first navigation (#916). The configured dev URL describes the
//! intended server when no document has been observed. `unserved_page` does
//! not grant navigation; `links` remains the owner of that decision.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::Url;

/// The scheme the explanation is served on.
pub const SCHEME: &str = "nessa-status";
/// The host the same scheme is served as on Windows.
pub const HTTP_HOST: &str = "nessa-status.localhost";
pub const FAILURE_PATH: &str = "/load-failure";

struct Page {
    app_started: bool,
    last_started: Url,
}

/// Facts from `on_page_load::Started`, keyed by the host's window label.
#[derive(Clone)]
pub struct Loads(Arc<Mutex<HashMap<String, Page>>>);

impl Loads {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Record the callback's URL, including blank and failure documents.
    /// Once an app page starts, later callbacks do not erase that fact.
    pub fn observe(&self, label: &str, url: &str) {
        let Ok(url) = Url::parse(url) else {
            return;
        };
        if let Ok(mut pages) = self.0.lock() {
            let app_started =
                is_app_page(&url) || pages.get(label).is_some_and(|page| page.app_started);
            pages.insert(
                label.to_owned(),
                Page {
                    app_started,
                    last_started: url,
                },
            );
        }
    }

    /// The page to name in a load diagnostic, or `None` when the app has
    /// started or the explanation is already loading. This uses published
    /// facts rather than `WebviewWindow::url`, whose Wry macOS implementation
    /// unwraps a missing `WKWebView.URL` before it can return an error.
    pub fn unserved_page(&self, label: &str, configured_page: &str) -> Option<String> {
        if let Ok(pages) = self.0.lock() {
            if let Some(page) = pages.get(label) {
                if page.app_started || is_load_failure_page(&page.last_started, true) {
                    return None;
                }
                if page.last_started.as_str() != "about:blank" {
                    return Some(page.last_started.to_string());
                }
            }
        }
        Some(configured_page.to_owned())
    }
}

/// Whether a page-load event names this app's existing served origins.
fn is_app_page(url: &Url) -> bool {
    matches!(
        (url.scheme(), url.host_str(), url.port()),
        ("tauri", Some("localhost"), None)
            | ("http", Some("tauri.localhost"), None)
            | (
                "http",
                Some("localhost" | "127.0.0.1" | "[::1]"),
                Some(1420)
            )
    )
}

/// The explanation page, admitted only when serving from a dev server.
pub fn is_load_failure_page(url: &Url, dev_server: bool) -> bool {
    if !dev_server || url.path() != FAILURE_PATH || url.port().is_some() {
        return false;
    }
    (url.scheme() == SCHEME && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some(HTTP_HOST))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_page_counts_and_a_blank_document_does_not() {
        let loads = Loads::new();
        loads.observe("main", "about:blank");
        assert_eq!(
            loads.unserved_page("main", "configured").as_deref(),
            Some("configured")
        );
        loads.observe("main", "nessa-status://localhost/load-failure");
        assert!(loads.unserved_page("main", "configured").is_none());
        loads.observe("main", "http://localhost:1420/index.html");
        assert!(loads.unserved_page("main", "configured").is_none());
        assert!(is_app_page(&Url::parse("http://127.0.0.1:1420/").unwrap()));
        assert!(is_app_page(
            &Url::parse("http://[::1]:1420/desktop.html").unwrap()
        ));
        assert!(is_app_page(
            &Url::parse("tauri://localhost/index.html").unwrap()
        ));
        assert!(is_app_page(
            &Url::parse("http://tauri.localhost/index.html").unwrap()
        ));
        assert!(!is_app_page(
            &Url::parse("http://localhost:7420/health").unwrap()
        ));
    }

    #[test]
    fn the_explanation_is_only_that_page_and_only_for_a_dev_server() {
        let page = Url::parse("nessa-status://localhost/load-failure?page=x").unwrap();
        assert!(is_load_failure_page(&page, true));
        assert!(!is_load_failure_page(&page, false));
        let other = Url::parse("nessa-status://localhost/other").unwrap();
        assert!(!is_load_failure_page(&other, true));
        let port = Url::parse("nessa-status://localhost:9/load-failure").unwrap();
        assert!(!is_load_failure_page(&port, true));
    }

    #[test]
    fn no_page_event_uses_the_configured_origin_including_an_override() {
        let loads = Loads::new();
        let configured = "http://127.0.0.1:1498/fixture?surface=desktop";
        assert_eq!(
            loads.unserved_page("desktop", configured).as_deref(),
            Some(configured)
        );
        loads.observe("desktop", "about:blank");
        assert_eq!(
            loads.unserved_page("desktop", configured).as_deref(),
            Some(configured)
        );
    }

    #[test]
    fn failure_pages_are_preserved_without_marking_an_app_page_loaded() {
        for url in [
            "nessa-status://localhost/load-failure?page=first",
            "http://nessa-status.localhost/load-failure?page=first",
        ] {
            let loads = Loads::new();
            loads.observe("desktop", url);
            assert_eq!(loads.unserved_page("desktop", "configured"), None);
            assert_eq!(
                loads.unserved_page("main", "configured").as_deref(),
                Some("configured")
            );
            loads.observe("desktop", "about:blank");
            assert_eq!(
                loads.unserved_page("desktop", "configured").as_deref(),
                Some("configured")
            );
        }
    }

    #[test]
    fn observed_unserved_pages_retain_their_url_and_loaded_windows_stay_loaded() {
        let loads = Loads::new();
        let observed = loads.clone();
        observed.observe("main", "http://localhost:1498/index.html?surface=setup");
        assert_eq!(
            loads.unserved_page("main", "configured").as_deref(),
            Some("http://localhost:1498/index.html?surface=setup")
        );
        observed.observe("main", "http://localhost:1420/index.html");
        observed.observe("main", "about:blank");
        assert_eq!(loads.unserved_page("main", "configured"), None);
        assert_eq!(
            Loads::new().unserved_page("main", "configured").as_deref(),
            Some("configured")
        );
    }
}
