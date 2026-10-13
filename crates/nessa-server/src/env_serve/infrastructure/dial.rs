//! How a paired environment reaches its gateway (issue #706): it dials the
//! gateway's native listener, as a paired device does, and the connection it
//! opens carries lease frames once the gateway agrees to be served.
//!
//! ```text
//! TCP connect, direct then through the relay ──▶ TLS: this environment's key, the gateway's key pinned
//!   ──▶ openEnvironment ──▶ serving ──▶ the stream lease frames ride
//!                       ──▶ refused ──▶ PermissionDenied: not served now
//! ```
//!
//! A refusal is redacted: the key holds no credential to serve, or the
//! gateway had no room or could not read its registry just then. The dialer
//! cannot tell them apart, so it tries a refusal again, with backoff.
//!
//! With a relay (`nessa env dial --relay`), the connect is the retained
//! reader's: direct first, and the relay asked for the gateway's key once
//! the direct dial has not connected within a short head start or failed
//! (`docs/design/auth/relay.md`). What it joins is the same pinned TLS, so a
//! relay decides nothing about which gateway is served.
//!
//! Arrows are steps, in order. The connect, the handshake and the answer are
//! each bounded by the native phase deadlines, on a blocking worker as the
//! device's enrollment client runs them; the stream is then the async
//! [`TlsStream`], which frames nothing, so lease frames keep their own bound.
use crate::device_pairing::infrastructure::TlsStream;
use nessa_auth::adapters::pairing::{
    GatewayTrust, NativeIdentity, NativeTransport, PairingCryptoError,
};
use nessa_client_core::retained::dial_gateway;
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::socket::{
    begin_enrollment_phase, DeadlineError, DeadlineStream, WakeEndpoint, TLS_DEADLINE,
};
use nessa_protocol::pairing::{
    wire::{decode_reply, encode_request, NativePairingReply, NativePairingRequest},
    EnrollmentChannel, NativeFrameError,
};
use nessa_protocol::relay::RelayAddress;
use std::{
    io::{Error, ErrorKind, Result as IoResult},
    sync::Arc,
};

/// Open one connection to the gateway at `address` (`host:port`, looked up
/// now and each address tried), whose key is `pin`, as the paired
/// environment `identity` is, and have it served. With a `relay`, the
/// gateway is asked for there when the direct dial does not connect.
///
/// # Errors
/// The gateway could not be reached, directly or through the relay, or
/// did not answer in time, the
/// handshake failed, as it does when its key is not `pin` or it offers no
/// protocol in common, as across an upgrade (`ConnectionAborted`), or cannot
/// succeed by trying again (`InvalidInput`: `pin` is not a key, or this
/// build cannot make the TLS setup), it refused to be served on this connection
/// (`PermissionDenied`, which may pass: see the module's note), or it
/// answered something else (`InvalidData`).
pub(crate) async fn dial(
    address: String,
    relay: Option<RelayAddress>,
    identity: Arc<NativeIdentity>,
    pin: [u8; 44],
    clock: Arc<dyn Clock>,
) -> IoResult<TlsStream> {
    let transport = tokio::task::spawn_blocking(move || {
        open_environment(&address, relay.as_ref(), &identity, pin, clock)
    })
    .await
    .map_err(|_| Error::other("the dialing worker failed"))??;
    // The socket is moved to the runtime here, where there is one.
    TlsStream::open(transport)
}

fn open_environment(
    address: &str,
    relay: Option<&RelayAddress>,
    identity: &NativeIdentity,
    pin: [u8; 44],
    clock: Arc<dyn Clock>,
) -> IoResult<NativeTransport<DeadlineStream>> {
    let socket = dial_gateway(address, relay.map(|relay| (relay, &pin)), TLS_DEADLINE)?;
    // Through a relay, the peer is the relay: the endpoint only names it.
    let peer = socket.peer_addr()?;
    let (stream, deadline) =
        DeadlineStream::new(socket, clock, WakeEndpoint::new(peer)).map_err(deadline_error)?;
    stream.blocking().map_err(deadline_error)?;
    // The handshake does not tell a cut connection from a key that is not
    // `pin`, or from a gateway with no protocol in common (rustls fails the
    // handshake for it); each is worth trying again, the last once the
    // gateway is upgraded. A pin that does not parse, or a TLS setup this
    // build cannot make, is not changed by dialing again.
    let transport = NativeTransport::connect(stream, identity, GatewayTrust::Pinned(pin))
        .map_err(handshake_error)?;
    begin_enrollment_phase(&deadline).map_err(deadline_error)?;
    let mut channel = EnrollmentChannel::new(transport);
    let request = encode_request(&NativePairingRequest::OpenEnvironment)
        .map_err(|error| Error::new(ErrorKind::InvalidData, format!("{error:?}")))?;
    channel.send_envelope(&request).map_err(frame_error)?;
    let reply = channel.receive_envelope().map_err(frame_error)?;
    match decode_reply(&reply) {
        Ok(NativePairingReply::Serving) => Ok(channel.into_transport()),
        Ok(NativePairingReply::Refused) => Err(Error::new(
            ErrorKind::PermissionDenied,
            "the gateway refused to be served on this connection",
        )),
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            "the gateway answered openEnvironment with something else",
        )),
    }
}

fn handshake_error(error: PairingCryptoError) -> Error {
    let kind = match error {
        PairingCryptoError::InvalidContext | PairingCryptoError::Unavailable => {
            ErrorKind::InvalidInput
        }
        _ => ErrorKind::ConnectionAborted,
    };
    Error::new(kind, format!("{error:?}"))
}

fn deadline_error(error: DeadlineError) -> Error {
    match error {
        DeadlineError::Io(kind) => Error::from(kind),
        DeadlineError::Phase => Error::new(ErrorKind::InvalidData, "a native phase out of order"),
    }
}

fn frame_error(error: NativeFrameError) -> Error {
    match error {
        NativeFrameError::Io(kind) => Error::from(kind),
        NativeFrameError::Wire(error) => Error::new(ErrorKind::InvalidData, format!("{error:?}")),
    }
}
