//! The ingress over real loopback sockets, with a semaphore standing in for
//! the listener's tunnel pool: what is upgraded, what is refused, and that
//! the bytes cross both ways.
use super::*;
use crate::tunnel::application::{MemoryDeviceSecrets, SecretRevocation};
use futures_util::{SinkExt, StreamExt};
use nessa_protocol::tunnel::{TunnelSecret, TUNNEL_SECRET_HEADER};
use std::io::{Read as _, Write as _};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

const REAL: Duration = Duration::from_secs(10);

struct Pool(Arc<Semaphore>);
impl TunnelAdmission<OwnedSemaphorePermit> for Pool {
    fn admit(&self) -> Option<OwnedSemaphorePermit> {
        self.0.clone().try_acquire_owned().ok()
    }
}

struct Serving {
    address: std::net::SocketAddr,
    pool: Arc<Semaphore>,
    secrets: Arc<MemoryDeviceSecrets>,
    paired: [u8; 32],
    joined: mpsc::Receiver<Tunneled<OwnedSemaphorePermit>>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

async fn serve(permits: usize) -> Serving {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let pool = Arc::new(Semaphore::new(permits));
    let secrets = Arc::new(MemoryDeviceSecrets::new());
    let paired = secrets.issue("device-1", &[1; 32], "owner").unwrap();
    let (joined_tx, joined) = mpsc::channel(8);
    let (stop, stopped) = watch::channel(false);
    let ingress = TunnelIngress::new(
        listener,
        Arc::new(Pool(pool.clone())),
        secrets.clone(),
        joined_tx,
    );
    Serving {
        address,
        pool,
        secrets,
        paired,
        joined,
        stop,
        task: tokio::spawn(ingress.serve(stopped)),
    }
}

fn request(
    address: std::net::SocketAddr,
    secret: Option<&[u8; 32]>,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{address}/native")
        .into_client_request()
        .unwrap();
    if let Some(secret) = secret {
        request.headers_mut().insert(
            TUNNEL_SECRET_HEADER,
            HeaderValue::from_str(&TunnelSecret::new(*secret).encode()).unwrap(),
        );
    }
    request
}

impl Serving {
    async fn stop(self) {
        self.stop.send_replace(true);
        tokio::time::timeout(REAL, self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn a_websocket_on_the_native_path_reaches_the_listener_and_bytes_cross() {
    let mut serving = serve(4).await;
    let (mut socket, _) = tokio::time::timeout(
        REAL,
        tokio_tungstenite::connect_async(request(serving.address, Some(&serving.paired))),
    )
    .await
    .unwrap()
    .unwrap();
    let Tunneled { stream, permit } = tokio::time::timeout(REAL, serving.joined.recv())
        .await
        .unwrap()
        .expect("a tunneled connection");
    assert_eq!(
        serving.pool.available_permits(),
        3,
        "it holds a tunnel permit"
    );
    let mut stream = stream;
    socket
        .send(Message::Binary(b"hello gateway".to_vec().into()))
        .await
        .unwrap();
    let heard = tokio::task::spawn_blocking(move || {
        let mut heard = [0; 13];
        stream.read_exact(&mut heard).unwrap();
        stream.write_all(b"hello reader").unwrap();
        (heard, stream)
    })
    .await
    .unwrap();
    assert_eq!(&heard.0, b"hello gateway");
    let mut answer = Vec::new();
    while answer.len() < 12 {
        match tokio::time::timeout(REAL, socket.next()).await.unwrap() {
            Some(Ok(Message::Binary(bytes))) => answer.extend_from_slice(&bytes),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(answer, b"hello reader");
    drop(permit);
    assert_eq!(serving.pool.available_permits(), 4);
    drop(heard);
    serving.stop().await;
}

#[tokio::test]
async fn any_other_path_is_refused_and_reaches_nothing() {
    let mut serving = serve(4).await;
    let refused =
        tokio_tungstenite::connect_async(format!("ws://{}/session", serving.address)).await;
    match refused {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 404),
        other => panic!("expected a 404, got {other:?}"),
    }
    assert!(serving.joined.try_recv().is_err());
    // Its permit was given back once it was refused.
    tokio::time::timeout(REAL, async {
        while serving.pool.available_permits() != 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    serving.stop().await;
}

#[tokio::test]
async fn with_the_pool_full_a_paired_device_is_refused_without_a_bridge() {
    let mut serving = serve(0).await;
    let refused =
        tokio_tungstenite::connect_async(request(serving.address, Some(&serving.paired))).await;
    match refused {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 503),
        other => panic!("expected 503, got {other:?}"),
    }
    assert!(serving.joined.try_recv().is_err());
    serving.stop().await;
}

#[tokio::test]
async fn strangers_holding_connections_cannot_keep_a_paired_device_out() {
    let mut serving = serve(4).await;
    let mut held = Vec::new();
    for _ in 0..8 {
        let mut stream = tokio::net::TcpStream::connect(serving.address)
            .await
            .unwrap();
        // A complete upgrade with no secret, and the socket left open.
        tokio::io::AsyncWriteExt::write_all(
            &mut stream,
            b"GET /native HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
        held.push(stream);
    }
    let (_socket, _) = tokio::time::timeout(
        REAL,
        tokio_tungstenite::connect_async(request(serving.address, Some(&serving.paired))),
    )
    .await
    .unwrap()
    .unwrap();
    let Tunneled { permit, .. } = tokio::time::timeout(REAL, serving.joined.recv())
        .await
        .unwrap()
        .expect("the paired device is admitted");
    assert_eq!(
        serving.pool.available_permits(),
        3,
        "the eight strangers hold no permit"
    );
    drop(permit);
    drop(_socket);
    drop(held);
    serving.stop().await;
}

/// Connections that never finish the upgrade stay within the pre-auth
/// slots. The ones past that are closed, and none of them takes a tunnel permit.
#[tokio::test]
async fn incomplete_upgrades_stay_within_the_preauth_slots() {
    let mut serving = serve(4).await;
    let mut held = Vec::new();
    for _ in 0..(super::PREAUTH_CAPACITY + 8) {
        let stream = tokio::net::TcpStream::connect(serving.address)
            .await
            .unwrap();
        held.push(stream);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let (open, closed) = loop {
        let mut open = 0;
        let mut closed = 0;
        for stream in &held {
            let mut buf = [0u8; 8];
            match stream.try_read(&mut buf) {
                Ok(0) => closed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => open += 1,
                Ok(_) => open += 1,
                Err(_) => closed += 1,
            }
        }
        if closed >= 8 && open <= super::PREAUTH_CAPACITY {
            break (open, closed);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "incomplete upgrades were not bounded: {open} open, {closed} closed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(
        open <= super::PREAUTH_CAPACITY,
        "{open} incomplete upgrades stayed open"
    );
    assert!(closed >= 8, "the extras were not closed: {closed}");
    assert_eq!(
        serving.pool.available_permits(),
        4,
        "an unfinished upgrade takes no tunnel permit"
    );
    drop(held);
    let (_socket, _) = tokio::time::timeout(
        REAL,
        tokio_tungstenite::connect_async(request(serving.address, Some(&serving.paired))),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(tokio::time::timeout(REAL, serving.joined.recv())
        .await
        .unwrap()
        .is_some());
    serving.stop().await;
}

#[tokio::test]
async fn a_revoked_device_is_rejected_and_takes_no_permit() {
    let mut serving = serve(4).await;
    serving
        .secrets
        .revoke("device-1", SecretRevocation::CredentialRevoked, "owner")
        .unwrap();
    let refused =
        tokio_tungstenite::connect_async(request(serving.address, Some(&serving.paired))).await;
    match refused {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 401),
        other => panic!("expected 401, got {other:?}"),
    }
    assert!(serving.joined.try_recv().is_err());
    assert_eq!(serving.pool.available_permits(), 4);
    // The path form of a secret that was never issued is the same refusal.
    let path = format!(
        "ws://{}/native/{}",
        serving.address,
        TunnelSecret::new(serving.paired).encode()
    );
    let refused = tokio_tungstenite::connect_async(path).await;
    match refused {
        Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 401),
        other => panic!("expected 401, got {other:?}"),
    }
    serving.stop().await;
}

#[tokio::test]
async fn a_secret_in_the_path_admits_when_the_header_is_absent() {
    let mut serving = serve(4).await;
    let path = format!(
        "ws://{}/native/{}",
        serving.address,
        TunnelSecret::new(serving.paired).encode()
    );
    let (_socket, _) = tokio::time::timeout(REAL, tokio_tungstenite::connect_async(path))
        .await
        .unwrap()
        .unwrap();
    assert!(tokio::time::timeout(REAL, serving.joined.recv())
        .await
        .unwrap()
        .is_some());
    serving.stop().await;
}

#[tokio::test]
async fn with_no_tunnel_the_listening_socket_alone_is_waited_on() {
    struct Never;
    impl EnrollmentAccept for Never {
        async fn accept(&mut self) -> Accepted {
            std::future::pending().await
        }
    }
    let (joined, receiver) = mpsc::channel::<Tunneled>(1);
    let mut accept = TunneledAccept::new(Never, receiver);
    // The tunnel's sender is gone (no tunnel, or it stopped): the closed
    // source is not answered as a connection, and accept keeps waiting.
    drop(joined);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), accept.accept())
            .await
            .is_err()
    );
    assert!(!accept.open);
}
