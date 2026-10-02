//! The sandbox proxy MCP Apps are drawn through (ADR 344, #349), served on a
//! scheme of its own so it is never the window's origin.
//!
//! An MCP App is someone else's HTML. The window frames this proxy, and the
//! proxy frames the app; the spec requires the proxy to sit on another origin
//! than the host, so that nothing the app runs can reach the window's storage
//! or its commands. `nessa-sandbox://localhost` (an `http` host,
//! `http://nessa-sandbox.localhost`, on Windows, where WebView2 serves a
//! custom scheme so) is that origin. It serves one document, the proxy
//! (`src/desktop/widgets/app/sandbox/proxy.html`), and nothing else: no app
//! page, no asset, no command. The browser build serves the same file from a
//! listener of its own (`src/desktop/widgets/app/sandbox/serve.ts`).
//!
//! ```text
//! window (tauri://) ──frames──▶ nessa-sandbox://localhost/proxy.html ──frames──▶ the app (srcdoc, opaque origin)
//! ```
//!
//! An arrow is an `<iframe>`. The window's CSP names this scheme in
//! `frame-src` (`tauri.conf.json`), and the navigation policy lets the frame
//! load (`links.rs`).

use std::borrow::Cow;

use tauri::http::{header, HeaderValue, Method, Response, StatusCode};

/// The scheme the proxy is served on.
pub const SCHEME: &str = "nessa-sandbox";

/// The host the same scheme is served as on Windows, where WebView2 serves a
/// custom scheme as an `http` host named after it.
pub const HTTP_HOST: &str = "nessa-sandbox.localhost";

/// The one path served.
const PROXY_PATH: &str = "/proxy.html";

/// The proxy, as the browser build serves it too: one file, two transports.
const PROXY: &str = include_str!("../../src/desktop/widgets/app/sandbox/proxy.html");

/// What the scheme answers a request with: the proxy for `GET /proxy.html`,
/// and nothing for anything else.
pub fn respond(method: &Method, path: &str) -> Response<Cow<'static, [u8]>> {
    if method != Method::GET || path != PROXY_PATH {
        let mut response = Response::new(Cow::Borrowed(&b"Not found"[..]));
        *response.status_mut() = StatusCode::NOT_FOUND;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        return response;
    }
    let mut response = Response::new(Cow::Borrowed(PROXY.as_bytes()));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use tauri::http::{header, Method, StatusCode};

    use super::{respond, HTTP_HOST, PROXY_PATH, SCHEME};

    #[test]
    fn the_proxy_is_served_as_html_and_never_cached() {
        let response = respond(&Method::GET, "/proxy.html");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = String::from_utf8(response.body().to_vec()).expect("the proxy is UTF-8");
        assert!(body.contains("ui/notifications/sandbox-proxy-ready"));
        // The app's frame is sandboxed without allow-same-origin.
        assert!(body.contains(r#"app.setAttribute("sandbox", "allow-scripts")"#));
    }

    /// The window and the browser build spell the proxy's address from these
    /// constants, not their own: this is what holds them to it.
    #[test]
    fn every_other_spelling_of_the_address_is_this_one() {
        let origins = include_str!("../../src/desktop/widgets/app/adapters/dom/sandbox-origin.ts");
        for spelled in [
            format!("{SCHEME}://localhost{PROXY_PATH}"),
            format!("{SCHEME}://localhost\""),
            format!("http://{HTTP_HOST}{PROXY_PATH}"),
            format!("http://{HTTP_HOST}\""),
        ] {
            assert!(
                origins.contains(&spelled),
                "sandbox-origin.ts lacks {spelled}"
            );
        }
        let listener = include_str!("../../src/desktop/widgets/app/sandbox/serve.ts");
        assert!(listener.contains(&format!("\"{PROXY_PATH}\"")));
        let config = include_str!("../tauri.conf.json");
        assert!(config.contains(&format!("frame-src {SCHEME}: http://{HTTP_HOST};")));
    }

    #[test]
    fn nothing_else_is_served() {
        for (method, path) in [
            (Method::GET, "/"),
            (Method::GET, "/index.html"),
            (Method::GET, "/proxy.html/"),
            (Method::GET, "/../index.html"),
            (Method::POST, "/proxy.html"),
        ] {
            assert_eq!(
                respond(&method, path).status(),
                StatusCode::NOT_FOUND,
                "{method} {path}"
            );
        }
    }
}
