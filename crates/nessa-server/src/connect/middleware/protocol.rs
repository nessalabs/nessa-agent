use crate::protocol::{ConnectParams, ResponseFrame, PROTOCOL_VERSION};

/// Reject connect when the client does not negotiate protocol v1.
///
/// Returns `Some(error frame)` when validation fails, `None` when the request may proceed.
pub fn validate_protocol_range(request_id: &str, params: &ConnectParams) -> Option<ResponseFrame> {
    if params.min_protocol > PROTOCOL_VERSION || params.max_protocol < PROTOCOL_VERSION {
        return Some(ResponseFrame::failure(
            request_id,
            "protocol_mismatch",
            "unsupported protocol version",
        ));
    }

    None
}
