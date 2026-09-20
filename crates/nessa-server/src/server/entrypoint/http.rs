use crate::agents::entrypoint::http as agents_handler;
use crate::attachments::entrypoint::http as attachments_handler;
use crate::browser_session::entrypoint as browser;
use crate::health::entrypoint::handler as health_handler;
use crate::protocol::MAX_PAYLOAD_BYTES;
use crate::server::entrypoint::origin;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put, Router};

/// Every RPC path uses the same mandatory authentication and authorization flow.
/// The HTTP probe reports liveness only and exposes no product state.
pub fn router(product: crate::product::ProductRouteState) -> Router {
    Router::new()
        .route("/health", get(health_handler::handle_http_health))
        // Asked while Nessa is being set up, when there is no session yet and
        // nothing to authenticate with. Discloses which agents could start
        // here and nothing else — no paths, no accounts, never a credential.
        .route(
            "/onboarding/agents",
            get(agents_handler::handle_http_agents),
        )
        .route("/session", get(product_upgrade))
        .route("/browser/login", post(browser::login))
        .route("/browser/check", post(browser::check))
        .route("/browser/logout", post(browser::logout))
        .route("/browser/session", get(browser_upgrade))
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024))
        // Added after the limit above so that limit does not apply to it. An
        // upload is bounded by its ticket instead: the body is streamed, and
        // abandoned as soon as it runs past the size the ticket was issued for.
        .route(
            "/attachments",
            put(attachments_handler::handle_upload)
                .options(attachments_handler::handle_preflight)
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .with_state(product)
}

async fn product_upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<crate::product::ProductRouteState>,
) -> Response {
    if headers
        .get(header::ORIGIN)
        .is_some_and(|value| !origin::is_trusted_ws_origin(value))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(MAX_PAYLOAD_BYTES as usize)
        .max_frame_size(MAX_PAYLOAD_BYTES as usize)
        .on_upgrade(move |socket| crate::product::handle_socket(socket, state))
        .into_response()
}

async fn browser_upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(mut state): State<crate::product::ProductRouteState>,
) -> Response {
    let session = match (&state.browser_sessions, browser::cookie(&headers)) {
        (Some(store), Some(id)) => match tokio::time::timeout(
            state.settings.handshake_timeout(),
            crate::browser_session::application::ReadBrowserSession {
                store: store.as_ref(),
            }
            .execute(id, state.clock.unix_seconds()),
        )
        .await
        {
            Ok(Ok(session)) => session,
            Ok(Err(_)) | Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
        _ => None,
    };
    let Some(session) = session else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if browser::origin(&headers, state.browser_http_allowed) != Some(session.origin()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    state.browser_session_id = browser::cookie(&headers).map(str::to_owned);
    state.browser_session_origin =
        browser::origin(&headers, state.browser_http_allowed).map(str::to_owned);
    ws.max_message_size(MAX_PAYLOAD_BYTES as usize)
        .max_frame_size(MAX_PAYLOAD_BYTES as usize)
        .on_upgrade(move |socket| crate::product::handle_socket(socket, state))
        .into_response()
}
