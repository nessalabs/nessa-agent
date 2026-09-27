use crate::core::trusted_origin::is_trusted_origin_value;
use axum::http::HeaderValue;

/// Accept WebSocket upgrades from native clients (no Origin) or loopback browser origins.
pub fn is_trusted_ws_origin(origin: &HeaderValue) -> bool {
    let Ok(value) = origin.to_str() else {
        return false;
    };

    is_trusted_origin_value(value)
}
