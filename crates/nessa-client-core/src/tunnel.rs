//! The bridge both ends of a web tunnel run (`docs/design/auth/tunnel.md`):
//! one WebSocket joined to one end of a loopback TCP pair, so what runs over
//! the other end is the same TCP stream a direct dial gives, and the native
//! TLS, deadlines and stop on it stay as they are.
//!
//! ```text
//! WebSocket ◀── binary messages ──▶ bridge ◀── bytes ──▶ loopback socket (ours)
//!                                                      ▲
//!                       the paired loopback socket ────┘ handed to the native code
//! ```
//!
//! Arrows are byte flow. The bridge decides nothing: it copies until either
//! side ends, then closes the other. It pings every [`TUNNEL_PING`] so an
//! edge's idle limit never ends a quiet session.
use futures_util::{SinkExt, StreamExt};
use nessa_protocol::relay::{
    tunnel_lookup_mac, GatewayKey, PublishedOrigin, RelayAddress, RelayFrame,
};
use nessa_protocol::tunnel::{
    TunnelAddress, TunnelSecret, MAX_TUNNEL_MESSAGE, TUNNEL_CHUNK, TUNNEL_PING,
    TUNNEL_SECRET_HEADER,
};
use std::{
    io::{Error, ErrorKind, Result as IoResult},
    net::{Ipv4Addr, SocketAddr},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_tungstenite::{
    tungstenite::{
        client::IntoClientRequest,
        http::HeaderValue,
        protocol::{Message, WebSocketConfig},
    },
    WebSocketStream,
};

/// The WebSocket limits both ends hold: no message or frame larger than
/// [`MAX_TUNNEL_MESSAGE`], and writes sent as they are made.
pub fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_TUNNEL_MESSAGE))
        .max_frame_size(Some(MAX_TUNNEL_MESSAGE))
        .write_buffer_size(0)
}

/// Copy between `websocket` and `socket` until either ends, then close
/// both. A text message, or anything over the limits, ends the bridge: the
/// native connection is binary and bounded, so neither is ours.
pub async fn bridge<S>(websocket: WebSocketStream<S>, socket: TcpStream)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut messages) = websocket.split();
    let (mut from_socket, mut to_socket) = socket.into_split();
    let inbound = async {
        while let Some(Ok(message)) = messages.next().await {
            match message {
                Message::Binary(bytes) => {
                    if to_socket.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                // Pongs are answered by the WebSocket itself as it reads.
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Text(_) | Message::Close(_) | Message::Frame(_) => break,
            }
        }
    };
    let outbound = async {
        let mut buffer = vec![0u8; TUNNEL_CHUNK];
        let mut ping =
            tokio::time::interval_at(tokio::time::Instant::now() + TUNNEL_PING, TUNNEL_PING);
        loop {
            tokio::select! {
                read = from_socket.read(&mut buffer) => match read {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if sink.send(Message::binary(buffer[..count].to_vec())).await.is_err() {
                            break;
                        }
                    }
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        // Tell the other end the stream is over, as a TCP close would.
        let _ = sink.send(Message::Close(None)).await;
        let _ = sink.close().await;
    };
    // Whichever direction ends first ends the bridge; dropping the other
    // half closes the socket and the WebSocket's transport.
    tokio::select! {
        () = inbound => {}
        () = outbound => {}
    }
}

/// A connected loopback TCP pair: the end the bridge keeps and the end
/// handed to the native code. The listener is made for this pair alone and
/// gone when it returns; a connection to it that is not the one just made
/// (another local process racing for the port) is dropped, so nothing but
/// this process can be on the other end.
pub async fn loopback_pair(timeout: Duration) -> IoResult<(TcpStream, std::net::TcpStream)> {
    tokio::time::timeout(timeout, async {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let connected = TcpStream::connect(listener.local_addr()?).await?;
        let ours = connected.local_addr()?;
        loop {
            let (accepted, from) = listener.accept().await?;
            if from == ours {
                connected.set_nodelay(true)?;
                accepted.set_nodelay(true)?;
                // Handed out blocking, as a dialed socket is.
                let theirs = connected.into_std()?;
                theirs.set_nonblocking(false)?;
                return Ok((accepted, theirs));
            }
        }
    })
    .await
    .map_err(|_| Error::from(ErrorKind::TimedOut))?
}

/// Open the tunnel at `address` within `timeout` and return the loopback
/// socket bridged to it: the native connection to the gateway runs over it
/// as over a direct dial's socket. The bridge runs on a thread of its own
/// until either side ends; dropping the returned socket ends it.
///
/// # Errors
/// The tunnel could not be reached, its TLS (for `https://`) or WebSocket
/// upgrade failed, or none of it finished within `timeout`.
pub fn dial(
    address: &TunnelAddress,
    secret: &TunnelSecret,
    timeout: Duration,
) -> IoResult<std::net::TcpStream> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
    let address = address.clone();
    let secret = *secret;
    let (sender, opened) = mpsc::channel();
    thread::Builder::new()
        .name("nessa-tunnel".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            };
            runtime.block_on(async move {
                let left = deadline.saturating_duration_since(Instant::now());
                let joined = tokio::time::timeout(left, open(&address, secret)).await;
                let (websocket, ours, theirs) = match joined {
                    Ok(Ok(joined)) => joined,
                    Ok(Err(error)) => {
                        let _ = sender.send(Err(error));
                        return;
                    }
                    Err(_) => {
                        let _ = sender.send(Err(Error::from(ErrorKind::TimedOut)));
                        return;
                    }
                };
                // A caller that gave up drops the socket, which ends the bridge.
                if sender.send(Ok(theirs)).is_ok() {
                    bridge(websocket, ours).await;
                }
            });
        })?;
    opened
        .recv_timeout(timeout)
        .map_err(|_| Error::from(ErrorKind::TimedOut))?
}

type Opened = (
    WebSocketStream<Box<dyn Transport>>,
    TcpStream,
    std::net::TcpStream,
);

/// What a tunnel's WebSocket runs over: TCP, or TLS over TCP.
pub(crate) trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}

async fn open(address: &TunnelAddress, secret: TunnelSecret) -> IoResult<Opened> {
    let tcp = TcpStream::connect((address.host(), address.port())).await?;
    tcp.set_nodelay(true)?;
    let transport: Box<dyn Transport> = if address.secure() {
        Box::new(tls(address, tcp).await?)
    } else {
        Box::new(tcp)
    };
    let mut request = address
        .websocket_url()
        .into_client_request()
        .map_err(|error| Error::new(ErrorKind::InvalidInput, error))?;
    request.headers_mut().insert(
        TUNNEL_SECRET_HEADER,
        HeaderValue::from_str(&secret.encode())
            .map_err(|error| Error::new(ErrorKind::InvalidInput, error))?,
    );
    let (websocket, _) =
        tokio_tungstenite::client_async_with_config(request, transport, Some(websocket_config()))
            .await
            .map_err(|error| Error::new(ErrorKind::ConnectionRefused, error))?;
    let (ours, theirs) = loopback_pair(Duration::from_secs(5)).await?;
    Ok((websocket, ours, theirs))
}

/// TLS to the tunnel's edge, its certificate checked against the platform's
/// trust store. This only finds the edge: the gateway is proved by the
/// native TLS inside, pinned at pairing.
async fn tls(
    address: &TunnelAddress,
    tcp: TcpStream,
) -> IoResult<tokio_rustls::client::TlsStream<TcpStream>> {
    use rustls::pki_types::ServerName;
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| Error::new(ErrorKind::InvalidInput, error))?
        .with_platform_verifier()
        .map_err(|error| Error::new(ErrorKind::InvalidInput, error))?
        .with_no_client_auth();
    let name = ServerName::try_from(address.host().to_owned())
        .map_err(|error| Error::new(ErrorKind::InvalidInput, error))?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
}

/// Ask `relay` for the tunnel origin the gateway `gateway` has published.
/// `secret` proves this is a paired device; the relay learns the mac, not
/// the secret. The origin is returned only when `gateway` signed it. An
/// unsigned origin, or one signed by any other key, is an error, and the
/// caller does not dial it. `Ok(None)` is a paired device and no origin. A
/// refusal or a gateway that is not registered is an error, so a saved
/// address is not replaced with nothing. Every step shares `timeout`.
///
/// # Errors
/// The relay could not be reached, the device was not allowed to read the
/// origin, the origin was not signed by `gateway`, or the answer was not a
/// tunnel origin.
pub fn published_address(
    relay: &RelayAddress,
    gateway: &GatewayKey,
    secret: &TunnelSecret,
    timeout: Duration,
) -> IoResult<Option<TunnelAddress>> {
    use std::net::ToSocketAddrs;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
    let mut refused = None;
    let mut stream = None;
    for address in relay.as_str().to_socket_addrs()? {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| Error::from(ErrorKind::TimedOut))?;
        match std::net::TcpStream::connect_timeout(&address, left) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => refused = Some(error),
        }
    }
    let mut stream =
        stream.ok_or_else(|| refused.unwrap_or_else(|| Error::from(ErrorKind::NotFound)))?;
    stream.set_nodelay(true)?;
    let bound = |stream: &std::net::TcpStream| -> IoResult<()> {
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| Error::from(ErrorKind::TimedOut))?;
        stream.set_read_timeout(Some(left))?;
        stream.set_write_timeout(Some(left))?;
        Ok(())
    };
    bound(&stream)?;
    nessa_protocol::relay::write_frame(&mut stream, &RelayFrame::Lookup { key: *gateway })?;
    bound(&stream)?;
    let RelayFrame::Challenge { nonce } = nessa_protocol::relay::read_frame(&mut stream)? else {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "the relay did not ask for a seal",
        ));
    };
    let mac = tunnel_lookup_mac(secret.expose(), gateway, &nonce);
    bound(&stream)?;
    nessa_protocol::relay::write_frame(&mut stream, &RelayFrame::Seal { mac })?;
    bound(&stream)?;
    match nessa_protocol::relay::read_frame(&mut stream)? {
        RelayFrame::Located { origin } => verified_origin(gateway, &origin),
        RelayFrame::Unlisted | RelayFrame::Offline => Err(Error::new(
            ErrorKind::PermissionDenied,
            "the tunnel address was not published to this device",
        )),
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            "the relay answered the lookup with something else",
        )),
    }
}

/// The origin text, once `gateway` is shown to have signed it. Empty is no
/// origin. Anything else is refused, and is not dialed.
fn verified_origin(
    gateway: &GatewayKey,
    origin: &PublishedOrigin,
) -> IoResult<Option<TunnelAddress>> {
    if origin.as_bytes().is_empty() {
        return Ok(None);
    }
    let Some((signature, text)) = origin.signature_and_text() else {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "the published origin is not signed by the paired gateway",
        ));
    };
    if !nessa_auth::adapters::pairing::verify_tunnel_origin(gateway, text.as_bytes(), &signature) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "the published origin is not signed by the paired gateway",
        ));
    }
    TunnelAddress::try_from(text.to_owned())
        .map(Some)
        .map_err(|error| Error::new(ErrorKind::InvalidData, error))
}

#[cfg(test)]
#[path = "../tests/tunnel/bridge.rs"]
mod tests;
