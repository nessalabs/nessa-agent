use crate::app::state::AppState;
use crate::protocol::{ConnectParams, ResponseFrame};

/// Reject connect when the client token does not match server policy.
///
/// Returns `Some(error frame)` when validation fails, `None` when the request may proceed.
pub fn validate_auth_token(
    request_id: &str,
    params: &ConnectParams,
    state: &AppState,
) -> Option<ResponseFrame> {
    if params.auth.token != state.auth_token() {
        return Some(ResponseFrame::failure(
            request_id,
            "unauthorized",
            "invalid auth token",
        ));
    }

    None
}
