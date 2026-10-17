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
