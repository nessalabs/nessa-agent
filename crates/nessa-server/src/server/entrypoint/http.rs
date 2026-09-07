use crate::health::entrypoint::handler as health_handler;
use crate::protocol::MAX_PAYLOAD_BYTES;
use crate::server::entrypoint::origin;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, Router};

/// Every RPC path uses the same mandatory authentication and authorization flow.
/// The HTTP probe reports liveness only and exposes no product state.
pub fn router(product: crate::product::ProductRouteState) -> Router {
    Router::new()
        .route("/health", get(health_handler::handle_http_health))
        .route("/session", get(product_upgrade))
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
