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
use nessa_protocol::relay::{RelayAddress, DIRECT_HEAD_START};
use nessa_protocol::tunnel::TunnelAddress;
use std::{
    io::{Error, ErrorKind, Result as IoResult},
    net::ToSocketAddrs,
    sync::{mpsc, Arc},
    time::Duration,
};

/// Open one connection to the gateway at `address` (`host:port`, looked up
/// now and each address tried), whose key is `pin`, as the paired
/// environment `identity` is, and have it served. With a `relay`, the
/// gateway is asked for there when the direct dial does not connect, and
/// with a `tunnel`, through its web tunnel.
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
    tunnel: Option<TunnelAddress>,
    secret: Option<[u8; 32]>,
    identity: Arc<NativeIdentity>,
    pin: [u8; 44],
    clock: Arc<dyn Clock>,
) -> IoResult<TlsStream> {
    let transport = tokio::task::spawn_blocking(move || {
        // Each address the name resolves to gets a whole dial of its own,
        // its handshake included, as a TCP connect by name tries each: a
        // first address that connects but is not the pinned gateway does
        // not stop the next. The relay finds the gateway by its key, and the
        // tunnel by its own address, not by this name, so with either a name
        // that does not resolve here, or not within the direct leg's head
        // start, is still reached that way.
        let beyond = relay.is_some() || tunnel.is_some();
        let gateways: Vec<String> = if beyond {
            resolved_within(&address, DIRECT_HEAD_START)
        } else {
            address
                .to_socket_addrs()?
                .map(|gateway| gateway.to_string())
                .collect()
        };
        let secret = secret.map(nessa_protocol::tunnel::TunnelSecret::new);
        let routes = Routes {
            relay: relay.as_ref(),
            tunnel: tunnel.as_ref().zip(secret.as_ref()),
        };
        if gateways.is_empty() && beyond {
            return open_environment(&address, routes, &identity, pin, clock);
        }
        let mut failed = Error::new(ErrorKind::NotFound, "no address");
        for gateway in gateways {
            match open_environment(&gateway, routes, &identity, pin, clock.clone()) {
                Ok(transport) => return Ok(transport),
                Err(error) if error.kind() == ErrorKind::InvalidInput => return Err(error),
                Err(error) => failed = error,
            }
        }
        Err(failed)
    })
    .await
    .map_err(|_| Error::other("the dialing worker failed"))??;
    // The socket is moved to the runtime here, where there is one.
    TlsStream::open(transport)
}

/// The addresses `address` resolves to, or none when the lookup fails or
/// is not done within `wait`. A lookup still running is left to end on its
/// own thread: the system resolver cannot be stopped.
fn resolved_within(address: &str, wait: Duration) -> Vec<String> {
    let (found, lookup) = mpsc::channel();
    let name = address.to_owned();
    std::thread::spawn(move || {
        let addresses = name.to_socket_addrs().map(|resolved| {
            resolved
                .map(|gateway| gateway.to_string())
                .collect::<Vec<_>>()
        });
        // The dial may have stopped waiting: nothing then takes the answer.
        let _ = found.send(addresses);
    });
    match lookup.recv_timeout(wait) {
        Ok(Ok(addresses)) => addresses,
        Ok(Err(_)) | Err(_) => Vec::new(),
    }
}

/// The ways beyond the local network a dial may take.
#[derive(Clone, Copy)]
struct Routes<'a> {
    relay: Option<&'a RelayAddress>,
    tunnel: Option<(&'a TunnelAddress, &'a nessa_protocol::tunnel::TunnelSecret)>,
}

fn open_environment(
    address: &str,
    routes: Routes<'_>,
    identity: &NativeIdentity,
    pin: [u8; 44],
    clock: Arc<dyn Clock>,
) -> IoResult<NativeTransport<DeadlineStream>> {
    let socket = dial_gateway(
        address,
        routes.relay.map(|relay| (relay, &pin)),
        routes.tunnel,
        TLS_DEADLINE,
    )?;
    // Through a relay, the peer is the relay, and through a tunnel, this
    // machine's end of its bridge: the endpoint only names it.
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
