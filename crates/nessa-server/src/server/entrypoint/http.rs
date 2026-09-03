use crate::app::state::AppState;
use crate::health::entrypoint::handler as health_handler;
use crate::protocol::MAX_PAYLOAD_BYTES;
use crate::server::entrypoint::{origin, ws};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, Router};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler::handle_http_health))
        .route("/", get(ws_upgrade))
        .with_state(state)
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin::is_trusted_ws_origin(origin) {
            tracing::warn!(origin = ?origin, "rejected WebSocket upgrade from untrusted origin");
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    ws.max_message_size(MAX_PAYLOAD_BYTES as usize)
        .max_frame_size(MAX_PAYLOAD_BYTES as usize)
        .on_upgrade(move |socket| ws::handle_socket(socket, state))
        .into_response()
}
