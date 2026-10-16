//! The product session over a protected native connection.
//!
//! ```text
//! ProtectedConnection (frames) --> NativeSocket (product messages)
//!     --> socket::serve_session(proof = device credentials + TLS key proof)
//! ```
//! Arrows are wrapping and calls. Only the credential verifier differs from the
//! browser socket: the credential id presented in `session.authenticate` is
//! checked against the key this connection's TLS handshake proved. Every read
//! after ready is admitted by the same socket loop and passive-read owners
//! (design rows PR1, PR3, PR4).
use super::socket::{serve_session, SessionProof};
use super::state::ProductRouteState;
use crate::device_pairing::infrastructure::{ProtectedConnection, ProtectedSessions};
use axum::extract::ws::Message;
use axum::Error;
use futures_util::{Sink, Stream};
use nessa_auth::application::{
    pairing::DeviceConnectionProof,
    ports::{AccessError, CredentialEvidence, CredentialVerifier, PortFuture, VerifiedCredential},
};
use nessa_auth::domain::AudienceId;
use nessa_protocol::product::generated::product_event;
use nessa_protocol::product::handshake::accepted_termination;
use nessa_protocol::protocol::{EventFrame, OutgoingMessage};
use std::{
    future::Future,
    io::{Error as IoError, ErrorKind},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Verifies a device credential id against the key a live native connection
/// proved. The registry implements it; the product asks it.
pub trait DeviceCredentials: Send + Sync {
    /// The credential `evidence` names, if it is an active device credential
    /// for `audience` bound to exactly the key in `proof`.
    fn verify<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential>;
    /// Whether the key in `proof` holds any active device credential for
    /// `audience`; the same binding rule `verify` applies, asked without a
    /// named credential.
    fn holds_credential(
        &self,
        proof: &DeviceConnectionProof,
        audience: &AudienceId,
    ) -> Result<bool, AccessError>;
}

/// Product sessions for protected native connections, composed once.
pub struct NativeSessions {
    state: ProductRouteState,
    proof: Arc<DeviceProof>,
}
impl NativeSessions {
    /// Serve native product sessions with the composed product state and the
    /// registry's device credential verifier.
    pub fn new(state: ProductRouteState, credentials: Arc<dyn DeviceCredentials>) -> Self {
        Self {
            state,
            proof: Arc::new(DeviceProof(credentials)),
        }
    }
}
impl ProtectedSessions for NativeSessions {
    fn admits(&self, proof: &DeviceConnectionProof) -> Result<bool, AccessError> {
        self.proof.0.holds_credential(proof, &self.state.audience)
    }
    fn serve(&self, connection: ProtectedConnection) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let state = self.state.clone();
        let proof = self.proof.clone();
        Box::pin(async move {
            serve_session(NativeSocket::new(connection), state, proof.as_ref()).await;
        })
    }
}

struct DeviceProof(Arc<dyn DeviceCredentials>);
impl SessionProof<NativeSocket> for DeviceProof {
    fn verifier<'a>(
        &'a self,
        socket: &'a NativeSocket,
        _: &'a ProductRouteState,
    ) -> Box<dyn CredentialVerifier + 'a> {
        Box::new(ProvedKey {
            credentials: self.0.as_ref(),
            proof: socket.connection.device_proof(),
        })
    }
}
struct ProvedKey<'a> {
    credentials: &'a dyn DeviceCredentials,
    proof: &'a DeviceConnectionProof,
}
impl CredentialVerifier for ProvedKey<'_> {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        self.credentials.verify(self.proof, evidence, audience)
    }
}

/// Product messages over protected frames: one UTF-8 text message per frame.
/// A close ends the connection after the frames before it are written. When
/// its payload is a session termination, that payload is written as
/// `session.terminated` and then the socket closes.
struct NativeSocket {
    connection: ProtectedConnection,
    phase: Phase,
}
/// Open, flushing the termination frame, or shutting the socket down.
enum Phase {
    Open,
    Deliver,
    Shut,
}
impl NativeSocket {
    fn new(connection: ProtectedConnection) -> Self {
        Self {
            connection,
            phase: Phase::Open,
        }
    }
}
impl Stream for NativeSocket {
    type Item = Result<Message, Error>;
    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.connection)
            .poll_next(context)
            .map(|frame| {
                frame.map(|frame| {
                    let body = frame.map_err(Error::new)?;
                    String::from_utf8(body)
                        .map(|text| Message::Text(text.into()))
                        .map_err(|_| Error::new(IoError::from(ErrorKind::InvalidData)))
                })
            })
    }
}
impl Sink<Message> for NativeSocket {
    type Error = Error;
    fn poll_ready(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), Error>> {
        let this = self.get_mut();
        if !matches!(this.phase, Phase::Open) {
            return Poll::Ready(Err(Error::new(IoError::from(ErrorKind::NotConnected))));
        }
        Pin::new(&mut this.connection)
            .poll_ready(context)
            .map_err(Error::new)
    }
    fn start_send(self: Pin<&mut Self>, message: Message) -> Result<(), Error> {
        let this = self.get_mut();
        match message {
            Message::Text(text) => Pin::new(&mut this.connection)
                .start_send(text.as_str().as_bytes().to_vec())
                .map_err(Error::new),
            Message::Close(frame) => {
                let queued = frame
                    .as_ref()
                    .and_then(|frame| termination_frame(frame.reason.as_ref()))
                    .is_some_and(|bytes| Pin::new(&mut this.connection).start_send(bytes).is_ok());
                // A failed or absent termination still closes the socket.
                this.phase = if queued { Phase::Deliver } else { Phase::Shut };
                Ok(())
            }
            // The product writes text and close only; nothing else has a frame.
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => Ok(()),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), Error>> {
        let this = self.get_mut();
        if matches!(this.phase, Phase::Deliver) {
            match Pin::new(&mut this.connection).poll_flush(context) {
                Poll::Pending => return Poll::Pending,
                // Delivery failed: close anyway. The peer then sees EOF
                // without a termination, which stays an untyped disconnect.
                Poll::Ready(_) => this.phase = Phase::Shut,
            }
        }
        if matches!(this.phase, Phase::Shut) {
            return Pin::new(&mut this.connection)
                .poll_close(context)
                .map_err(Error::new);
        }
        Pin::new(&mut this.connection)
            .poll_flush(context)
            .map_err(Error::new)
    }
    fn poll_close(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.get_mut().connection)
            .poll_close(context)
            .map_err(Error::new)
    }
}

/// The text frame a native close carries when its reason is a termination.
/// Sequence zero: termination is not a conversation state change.
fn termination_frame(reason: &str) -> Option<Vec<u8>> {
    let payload = serde_json::from_str(reason).ok()?;
    let termination = accepted_termination(&payload)?;
    let text = OutgoingMessage::Event(
        EventFrame::push(product_event::SESSION_TERMINATED, &termination, 0, 0).ok()?,
    )
    .to_wire_text()
    .ok()?;
    Some(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::termination_frame;
    use nessa_protocol::product::generated::SessionTermination;
    use nessa_protocol::product_contract::generated::SessionCloseReason;
    use nessa_protocol::protocol::OutgoingMessage;

    #[test]
    fn a_close_reason_becomes_session_terminated_and_a_blank_close_does_not() {
        let termination = SessionTermination {
            code: SessionCloseReason::CredentialRevoked,
            retryable: false,
            retry_after_ms: None,
        };
        let bytes = termination_frame(&serde_json::to_string(&termination).unwrap()).unwrap();
        let OutgoingMessage::Event(event) =
            OutgoingMessage::decode(std::str::from_utf8(&bytes).unwrap()).unwrap()
        else {
            panic!("termination is an event");
        };
        assert_eq!(event.event, "session.terminated");
        assert_eq!(event.payload["code"], "credential_revoked");
        assert_eq!(event.payload["retryable"], false);
        assert!(termination_frame("").is_none());
        assert!(termination_frame(r#"{"code":"credential_revoked","retryable":true}"#).is_none());
    }
}
