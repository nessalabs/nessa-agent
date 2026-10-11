//! How a paired environment reaches its gateway (issue #706): it dials the
//! gateway's native listener, as a paired device does, and the connection it
//! opens carries lease frames once the gateway agrees to be served.
//!
//! ```text
//! TCP connect ──▶ TLS: this environment's key, the gateway's key pinned
//!   ──▶ openEnvironment ──▶ serving ──▶ the stream lease frames ride
//!                       ──▶ refused ──▶ PermissionDenied: this key may not serve
//! ```
//!
//! Arrows are steps, in order. The connect, the handshake and the answer are
//! each bounded by the native phase deadlines, on a blocking worker as the
//! device's enrollment client runs them; the stream is then the async
//! [`TlsStream`], which frames nothing, so lease frames keep their own bound.
use crate::device_pairing::infrastructure::TlsStream;
use nessa_auth::adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport};
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::socket::{
    begin_enrollment_phase, DeadlineError, DeadlineStream, WakeEndpoint, TLS_DEADLINE,
};
use nessa_protocol::pairing::{
    wire::{decode_reply, encode_request, NativePairingReply, NativePairingRequest},
    EnrollmentChannel, NativeFrameError,
};
use std::{
    io::{Error, ErrorKind, Result as IoResult},
    net::{SocketAddr, TcpStream},
    sync::Arc,
};

/// Open one connection to the gateway at `address`, whose key is `pin`, as
/// the paired environment `identity` is, and have it served.
///
/// # Errors
/// The gateway could not be reached or did not answer in time, the
/// handshake failed, as it does when its key is not `pin`
/// (`ConnectionAborted`), it refused this key (`PermissionDenied`), or it
/// answered something else (`InvalidData`).
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the environment's command dials in part 1c")
)]
pub(crate) async fn dial(
    address: SocketAddr,
    identity: Arc<NativeIdentity>,
    pin: [u8; 44],
    clock: Arc<dyn Clock>,
) -> IoResult<TlsStream> {
    let transport =
        tokio::task::spawn_blocking(move || open_environment(address, &identity, pin, clock))
            .await
            .map_err(|_| Error::other("the dialing worker failed"))??;
    // The socket is moved to the runtime here, where there is one.
    TlsStream::open(transport)
}

fn open_environment(
    address: SocketAddr,
    identity: &NativeIdentity,
    pin: [u8; 44],
    clock: Arc<dyn Clock>,
) -> IoResult<NativeTransport<DeadlineStream>> {
    let socket = TcpStream::connect_timeout(&address, TLS_DEADLINE)?;
    let (stream, deadline) =
        DeadlineStream::new(socket, clock, WakeEndpoint::new(address)).map_err(deadline_error)?;
    stream.blocking().map_err(deadline_error)?;
    // The handshake does not tell a cut connection from a key that is not
    // `pin`; either is worth trying again, unlike a refusal of this key.
    let transport = NativeTransport::connect(stream, identity, GatewayTrust::Pinned(pin))
        .map_err(|error| Error::new(ErrorKind::ConnectionAborted, format!("{error:?}")))?;
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
            "the gateway refused to be served by this key",
        )),
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            "the gateway answered openEnvironment with something else",
        )),
    }
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
