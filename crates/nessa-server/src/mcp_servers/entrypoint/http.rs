//! `GET /mcp-resources` (#348): an MCP App resource's held bytes, for the
//! ticket `mcp.readResource` answered with. The protocol's contract is
//! `protocol/README.md`, "An MCP App's calls".
use crate::mcp_servers::infrastructure::ResourceTicketStore;
use crate::server::entrypoint::origin::{self, allowed_origin, with_cors, Allowed};
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// The header that carries a ticket. A header, never a query parameter, so
/// the secret stays out of URLs, access logs, history, and referrers; this
/// route reads it from nowhere else.
pub const TICKET_HEADER: &str = "x-nessa-resource-ticket";

/// What a held resource is served as: an MCP App's HTML, by the profile the
/// MCP Apps extension names it with.
pub const CONTENT_TYPE: &str = "text/html;profile=mcp-app";

/// All the resource route is given: the ticket store, when MCP servers are
/// composed. It has no use for sessions, credentials, or conversations, and
/// cannot reach them.
#[derive(Clone)]
pub struct ResourceRoute {
    tickets: Option<Arc<ResourceTicketStore>>,
}
impl ResourceRoute {
    pub fn new(tickets: Option<Arc<ResourceTicketStore>>) -> Self {
        Self { tickets }
    }
}

/// `GET /mcp-resources`: the bytes one ticket holds, once.
///
/// The route authenticates nobody. The ticket is the whole authority: the
/// authenticated socket decided, under its policy and audit, that this app
/// may have these bytes, and this route learns only what the ticket says.
///
/// Every refusal of a ticket is the same `404` with no body
/// (`every_refused_ticket_is_the_same_empty_404`): missing, doubled, never
/// issued, spent, expired, released, or not a ticket at all. Which it was is
/// the audit trail's to know, not the holder's. A page on an origin this
/// server does not trust is refused `403` before the ticket is looked at, as
/// at `PUT /attachments`.
pub(crate) async fn handle_resource(
    State(route): State<ResourceRoute>,
    headers: HeaderMap,
) -> Response {
    let allowed = allowed_origin(&headers);
    if matches!(allowed, Allowed::No) {
        return with_cors(StatusCode::FORBIDDEN.into_response(), allowed);
    }
    let response = match &route.tickets {
        Some(tickets) => redeem(tickets, &headers),
        // No MCP server is composed, so nothing issued a ticket.
        None => not_found(),
    };
    with_cors(response, allowed)
}

/// `HEAD /mcp-resources` is refused like any spent ticket, never redeemed:
/// a ticket is single use, and an answer with no body would spend it.
pub(crate) async fn handle_head(headers: HeaderMap) -> Response {
    let allowed = allowed_origin(&headers);
    if matches!(allowed, Allowed::No) {
        return with_cors(StatusCode::FORBIDDEN.into_response(), allowed);
    }
    with_cors(not_found(), allowed)
}

fn redeem(tickets: &ResourceTicketStore, headers: &HeaderMap) -> Response {
    // Exactly one ticket. Two headers are not a choice this route makes.
    let mut presented = headers.get_all(TICKET_HEADER).iter();
    let (Some(ticket), None) = (presented.next(), presented.next()) else {
        return not_found();
    };
    match tickets.redeem(ticket.as_bytes()) {
        Some(resource) => {
            let mut response = Body::from(resource.bytes.to_vec()).into_response();
            let answer = response.headers_mut();
            answer.insert(header::CONTENT_TYPE, HeaderValue::from_static(CONTENT_TYPE));
            answer.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            answer.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            answer.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment"),
            );
            response
        }
        None => not_found(),
    }
}

/// The one refusal: `404`, no body.
fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// `OPTIONS /mcp-resources`: the same preflight as `PUT /attachments`, for a
/// `GET` carrying the ticket header.
pub(crate) async fn handle_preflight(headers: HeaderMap) -> Response {
    origin::preflight(&headers, "GET", TICKET_HEADER)
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/http.rs"]
mod tests;
