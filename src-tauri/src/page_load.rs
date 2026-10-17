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
use std::time::Duration;

use tauri::http::{header, HeaderValue, Method, Response, StatusCode};
use tauri::{AppHandle, Manager, Url};

use crate::host_refusal;

#[path = "page_load/state.rs"]
mod state;
use state::FAILURE_PATH;
pub use state::{is_load_failure_page, Loads, SCHEME};

pub fn load_failure_url(page: &str) -> Url {
    let mut url = Url::parse(&format!("{SCHEME}://localhost{FAILURE_PATH}"))
        .expect("load failure url is static");
    url.query_pairs_mut().append_pair("page", page);
    url
}

/// After a few seconds, any dev window that has not started loading its page
/// is sent to the explanation. A packaged build returns immediately: its
/// pages come from the app, and a missing asset is not a dev server.
pub fn watch(app: AppHandle, loads: Loads, windows: &'static [&'static str]) {
    if !cfg!(dev) {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || explain(&handle, &loads, windows)) {
            crate::host_log::warn(format!(
                "could not report a page that never loaded: {error}"
            ));
        }
    });
}

fn explain(app: &AppHandle, loads: &Loads, windows: &[&str]) {
    let configured_page = app
        .config()
        .build
        .dev_url
        .as_ref()
        .map(Url::as_str)
        .unwrap_or("unavailable (no configured development URL)");
    for &label in windows {
        let Some(page) = loads.unserved_page(label, configured_page) else {
            continue;
        };
        let Some(window) = app.get_webview_window(label) else {
            continue;
        };
        let said = host_refusal::fill("document-unserved", &[("page", page.as_str())]);
        tracing::warn!(target: "nessa.window", label, "{said}");
        if let Err(error) = window.navigate(load_failure_url(&page)) {
            tracing::warn!(
                target: "nessa.window",
                label,
                %error,
                "could not open the load failure page"
            );
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

fn document() -> String {
    let face = include_str!("../../src/host/startup-face.html")
        .replace("{{LINE}}", &escape(&host_refusal::line()))
        .replace(
            "{{CODE}}",
            &escape(&host_refusal::code("document-unserved")),
        );
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
  html, body {{ margin: 0; height: 100%; background: #000; }}
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
        assert!(!body.contains("data-nessa-startup-mark"), "{body}");
        assert!(!body.contains("data-nessa-load-mark"), "{body}");
        assert!(!body.contains("nessa-avatar"), "{body}");
        assert!(!body.contains("filter=\"url("), "{body}");
        assert!(body.contains("aria-label=\"Restart\""), "{body}");
        assert!(body.contains("aria-label=\"Quit\""), "{body}");
        assert!(!body.contains("not serving"), "{body}");
    }

    #[test]
    fn an_app_page_counts_and_a_blank_document_does_not() {
        let loads = Loads::new();
        loads.observe("main", "about:blank");
        assert_eq!(
            loads.unserved_page("main", "configured").as_deref(),
            Some("configured")
        );
        loads.observe("main", "nessa-status://localhost/load-failure");
        assert_eq!(loads.unserved_page("main", "configured"), None);
        loads.observe("main", "http://localhost:1420/index.html");
        assert_eq!(loads.unserved_page("main", "configured"), None);
        for page in [
            "http://127.0.0.1:1420/",
            "http://[::1]:1420/desktop.html",
            "tauri://localhost/index.html",
            "http://tauri.localhost/index.html",
        ] {
            let loads = Loads::new();
            loads.observe("main", page);
            assert_eq!(loads.unserved_page("main", "configured"), None, "{page}");
        }
        let loads = Loads::new();
        let foreign = "http://localhost:7420/health";
        loads.observe("main", foreign);
        assert_eq!(
            loads.unserved_page("main", "configured").as_deref(),
            Some(foreign)
        );
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
