use crate::core::trusted_origin::is_trusted_origin_value;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Accept WebSocket upgrades from native clients (no Origin) or loopback browser origins.
pub fn is_trusted_ws_origin(origin: &HeaderValue) -> bool {
    let Ok(value) = origin.to_str() else {
        return false;
    };

    is_trusted_origin_value(value)
}

/// How long a browser may remember a ticket route's preflight answer, in
/// seconds.
const PREFLIGHT_MAX_AGE: &str = "600";

/// Who asked a ticket route (`PUT /attachments`, `GET /mcp-resources`), and
/// whether they may send and read.
pub(crate) enum Allowed {
    /// No `Origin`: a native client, not a page.
    Same,
    /// A page on an origin this server trusts, echoed back exactly. Never `*`.
    Cross(HeaderValue),
    /// A page on any other origin.
    No,
}

/// The same rule `/session` applies: a present `Origin` must be one this
/// server trusts. A ticket is a bearer secret, and a page on another origin
/// holding one is not a caller this gateway serves.
pub(crate) fn allowed_origin(headers: &HeaderMap) -> Allowed {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return Allowed::Same;
    };
    if is_trusted_ws_origin(origin) {
        Allowed::Cross(origin.clone())
    } else {
        Allowed::No
    }
}

/// Every answer varies by origin, a refusal of an origin included, so no cache
/// hands one origin's answer to another. A trusted page is told it may read it.
pub(crate) fn with_cors(mut response: Response, allowed: Allowed) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::VARY, HeaderValue::from_static("origin"));
    if let Allowed::Cross(origin) = allowed {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    response
}

/// A ticket route's `OPTIONS`: a browser asks before it sends a request
/// carrying a header of its own from another origin, which is every request
/// from the desktop shell and from the development server. `methods` and
/// `request_headers` are what the route lets such a page send.
pub(crate) fn preflight(
    headers: &HeaderMap,
    methods: &'static str,
    request_headers: &'static str,
) -> Response {
    let allowed = allowed_origin(headers);
    if matches!(allowed, Allowed::No) {
        return with_cors(StatusCode::FORBIDDEN.into_response(), allowed);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let answer = response.headers_mut();
    answer.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(methods),
    );
    answer.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(request_headers),
    );
    answer.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static(PREFLIGHT_MAX_AGE),
    );
    with_cors(response, allowed)
}
