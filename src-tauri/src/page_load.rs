//! What a dev window shows when its document never arrives.
//!
//! `tauri dev` opens the window, then waits on Vite. When Vite is not
//! listening, the webview stays on `about:blank` and a transparent panel
//! shows the desktop through it. This module names that: if a window's own
//! page has not started loading, the host navigates it to a document that
//! says which page was not served, in the log. The window shows the same
//! calm screen as every other startup failure.
//!
//! The scheme is registered in every build. [`crate::links`] allows the
//! navigation only while the dev server is the app's origin, so a packaged
//! window cannot be sent here. The page URL stays in the query and the log;
//! the document does not echo it. Its policy allows the inline style and
//! the inline actions, and nothing else.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::http::{header, HeaderValue, Method, Response, StatusCode};
use tauri::{AppHandle, Manager, Url};

use crate::host_refusal;

/// The scheme the explanation is served on.
pub const SCHEME: &str = "nessa-status";

/// The host the same scheme is served as on Windows.
pub const HTTP_HOST: &str = "nessa-status.localhost";

const FAILURE_PATH: &str = "/load-failure";

/// Windows whose first app-page load has been seen, by label.
#[derive(Clone)]
pub struct Loads(Arc<Mutex<HashSet<String>>>);

impl Loads {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashSet::new())))
    }

    /// Record that `label` started loading an app page. `about:blank` and the
    /// explanation itself do not count: a window that only reached those has
    /// not loaded.
    pub fn observe(&self, label: &str, url: &str) {
        if !is_app_page(url) {
            return;
        }
        if let Ok(mut seen) = self.0.lock() {
            seen.insert(label.to_owned());
        }
    }

    pub fn saw(&self, label: &str) -> bool {
        self.0
            .lock()
            .map(|seen| seen.contains(label))
            .unwrap_or(false)
    }
}

/// Whether `url` is a page this app serves, so a load of it means the dev
/// server (or the packaged protocol) answered.
fn is_app_page(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    matches!(
        (parsed.scheme(), parsed.host_str(), parsed.port()),
        ("tauri", Some("localhost"), None)
            | ("http", Some("tauri.localhost"), None)
            | (
                "http",
                Some("localhost" | "127.0.0.1" | "[::1]"),
                Some(1420)
            )
    )
}

/// The explanation page. `dev_server` is the build: a packaged window is
/// never allowed to navigate here.
pub fn is_load_failure_page(url: &Url, dev_server: bool) -> bool {
    if !dev_server || url.path() != FAILURE_PATH || url.port().is_some() {
        return false;
    }
    (url.scheme() == SCHEME && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some(HTTP_HOST))
}

pub fn load_failure_url(page: &str) -> Url {
    let mut url = Url::parse(&format!("{SCHEME}://localhost{FAILURE_PATH}"))
        .expect("load failure url is static");
    url.query_pairs_mut().append_pair("page", page);
    url
}

/// After a few seconds, any dev window that has not started loading its page
/// is sent to the explanation. A packaged build returns immediately: its
/// pages come from the app, and a missing asset is not a dev server.
pub fn watch(app: AppHandle, loads: Loads) {
    if !cfg!(dev) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || explain(&handle, &loads)) {
            eprintln!("[nessa] could not report a page that never loaded: {error}");
        }
    });
}

fn explain(app: &AppHandle, loads: &Loads) {
    for label in [
        crate::panel::MAIN_WINDOW,
        crate::panel::SETUP_WINDOW,
        crate::desktop_window::DESKTOP_WINDOW,
    ] {
        if loads.saw(label) {
            continue;
        }
        let Some(window) = app.get_webview_window(label) else {
            continue;
        };
        if window
            .url()
            .ok()
            .is_some_and(|url| is_load_failure_page(&url, true))
        {
            continue;
        }
        let page = window
            .url()
            .map(|url| url.to_string())
            .unwrap_or_else(|_| "http://localhost:1420/".to_string());
        let said = host_refusal::fill("document-unserved", &[("page", page.as_str())]);
        eprintln!("[nessa] {label}: {said}");
        if let Err(error) = window.navigate(load_failure_url(&page)) {
            eprintln!("[nessa] {label}: could not open the load failure page: {error}");
        }
    }
}

pub fn respond(method: &Method, path: &str, query: Option<&str>) -> Response<Cow<'static, [u8]>> {
    if method != Method::GET || path != FAILURE_PATH {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    // The page URL is the log's and the query's (`load_failure_url`). The
    // document does not read it back.
    let _ = query;
    let body = document();
    let mut response = Response::new(Cow::Owned(body.into_bytes()));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn plain(status: StatusCode, body: &'static str) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::new(Cow::Borrowed(body.as_bytes()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    response
}

fn avatar_mark() -> String {
    include_str!("../icons/nessa-avatar.svg").replacen("<svg ", "<svg data-nessa-startup-mark ", 1)
}

fn document() -> String {
    let face = include_str!("../../src/host/startup-face.html")
        .replace(
            r#"<img data-nessa-startup-mark src="/src-tauri/icons/nessa-avatar.svg" alt="" width="28" height="28" />"#,
            &avatar_mark(),
        )
        .replace("{{LINE}}", &escape(&host_refusal::line()))
        .replace("{{CODE}}", &escape(&host_refusal::code("document-unserved")));
    let css = include_str!("../../src/host/startup-screen.css");
    let actions = include_str!("../../src/host/startup-actions.js");
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
<title>Nessa</title>
<style>
  html, body {{ margin: 0; height: 100%; background: #121214; }}
  {css}
</style>
</head>
<body>
<main data-nessa-startup-screen data-nessa-startup-overlay role="alert">{face}</main>
<script>
{actions}
wireStartupActions(document.querySelector("[data-nessa-startup-screen]"));
</script>
</body>
</html>
"#
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::http::Method;

    fn page_from_query(query: Option<&str>) -> String {
        let raw = query.unwrap_or("");
        let page = Url::parse(&format!("http://localhost/?{raw}"))
            .ok()
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "page")
                    .map(|(_, value)| value.into_owned())
            })
            .filter(|page| !page.is_empty())
            .unwrap_or_else(|| "http://localhost:1420/".to_string());
        page.chars().take(2048).collect()
    }

    #[test]
    fn an_app_page_counts_and_a_blank_document_does_not() {
        let loads = Loads::new();
        loads.observe("main", "about:blank");
        loads.observe("main", "nessa-status://localhost/load-failure");
        assert!(!loads.saw("main"));
        loads.observe("main", "http://localhost:1420/index.html");
        assert!(loads.saw("main"));
        assert!(is_app_page("http://127.0.0.1:1420/"));
        assert!(is_app_page("http://[::1]:1420/desktop.html"));
        assert!(is_app_page("tauri://localhost/index.html"));
        assert!(is_app_page("http://tauri.localhost/index.html"));
        assert!(!is_app_page("http://localhost:7420/health"));
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
    fn the_page_round_trips_through_the_query_and_stays_off_the_screen() {
        let page = "http://localhost:1420/index.html?surface=setup";
        let url = load_failure_url(page);
        assert_eq!(page_from_query(url.query()), page);
        let response = respond(
            &Method::GET,
            FAILURE_PATH,
            Some("page=%3Cscript%3Ealert(1)%3C%2Fscript%3E"),
        );
        let body = String::from_utf8(response.body().to_vec()).unwrap();
        assert!(!body.contains("<script>alert"));
        assert!(!body.contains("&lt;script&gt;"));
        assert!(body.contains("default-src 'none'"));
        assert!(body.contains("STARTUP_PAGE"), "{body}");
        assert!(body.contains("data-nessa-startup-mark"), "{body}");
        assert!(body.contains("aria-label=\"Restart\""), "{body}");
        assert!(body.contains("aria-label=\"Quit\""), "{body}");
        assert!(!body.contains("not serving"), "{body}");
        assert!(
            !body.contains("/src-tauri/icons/nessa-avatar.svg"),
            "{body}"
        );
    }
}
