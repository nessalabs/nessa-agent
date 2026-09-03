use crate::protocol::{ConnectParams, ResponseFrame};
use crate::server::entrypoint::session::WsSession;

/// Reject connect when the client did not echo this socket's challenge nonce.
pub fn validate_challenge_nonce(
    request_id: &str,
    params: &ConnectParams,
    session: &WsSession,
) -> Option<ResponseFrame> {
    let Some(expected) = session.challenge_nonce() else {
        return Some(ResponseFrame::failure(
            request_id,
            "internal_error",
            "missing connect challenge",
        ));
    };

    if params.auth.nonce != expected {
        return Some(ResponseFrame::failure(
            request_id,
            "invalid_challenge",
            "challenge nonce mismatch",
        ));
    }

    None
}
