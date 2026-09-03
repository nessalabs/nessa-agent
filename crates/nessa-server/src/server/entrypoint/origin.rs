use axum::http::HeaderValue;

/// Accept WebSocket upgrades from native clients (no Origin) or loopback browser origins.
pub fn is_trusted_ws_origin(origin: &HeaderValue) -> bool {
    let Ok(value) = origin.to_str() else {
        return false;
    };

    is_trusted_origin_value(value)
}

pub fn is_trusted_origin_value(origin: &str) -> bool {
    origin.starts_with("http://127.0.0.1:")
        || origin.starts_with("http://localhost:")
        || origin.starts_with("https://127.0.0.1:")
        || origin.starts_with("https://localhost:")
        || origin == "tauri://localhost"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_untrusted_browser_origin() {
        assert!(!is_trusted_origin_value("https://evil.example"));
    }

    #[test]
    fn accepts_loopback_browser_origin() {
        assert!(is_trusted_origin_value("http://127.0.0.1:5173"));
    }
}
