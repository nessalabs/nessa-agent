use crate::app::state::AppState;
use crate::protocol::{ConnectParams, ResponseFrame};

/// Reject connect when the client token does not match server policy.
///
/// Returns `Some(error frame)` when validation fails, `None` when the request may proceed.
/// Token bytes are compared in constant time to avoid leaking length/prefix via timing.
pub fn validate_auth_token(
    request_id: &str,
    params: &ConnectParams,
    state: &AppState,
) -> Option<ResponseFrame> {
    if !constant_time_eq(params.auth.token.as_bytes(), state.auth_token().as_bytes()) {
        return Some(ResponseFrame::failure(
            request_id,
            "unauthorized",
            "invalid auth token",
        ));
    }

    None
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_bytes_match() {
        assert!(constant_time_eq(b"dev-token", b"dev-token"));
    }

    #[test]
    fn unequal_bytes_differ() {
        assert!(!constant_time_eq(b"dev-token", b"wrong"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
