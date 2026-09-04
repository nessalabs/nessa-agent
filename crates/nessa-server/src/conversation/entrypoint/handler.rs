use crate::protocol::{echo_message, EchoParams, OutgoingMessage, ResponseFrame};

/// Handle `conversation.echo`: echo the client text.
pub fn handle_echo(request_id: &str, params: EchoParams) -> OutgoingMessage {
    echo_message(request_id, params.text).unwrap_or_else(|error| {
        tracing::error!(%error, "failed to encode echo response");
        OutgoingMessage::Response(ResponseFrame::failure(
            request_id,
            "internal_error",
            "failed to encode response",
        ))
    })
}
