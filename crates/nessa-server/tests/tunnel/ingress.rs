//! The ingress over real loopback sockets, with a semaphore standing in for
//! the listener's tunnel pool: what is upgraded, what is refused, and that
//! the bytes cross both ways.
use super::*;
use futures_util::{SinkExt, StreamExt};
use std::io::{Read as _, Write as _};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::{self, Message};

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
    joined: mpsc::Receiver<Tunneled<OwnedSemaphorePermit>>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

async fn serve(permits: usize) -> Serving {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let pool = Arc::new(Semaphore::new(permits));
    let (joined_tx, joined) = mpsc::channel(4);
    let (stop, stopped) = watch::channel(false);
    let ingress = TunnelIngress::new(listener, Arc::new(Pool(pool.clone())), joined_tx);
    Serving {
        address,
        pool,
        joined,
        stop,
        task: tokio::spawn(ingress.serve(stopped)),
    }
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
        tokio_tungstenite::connect_async(format!("ws://{}/native", serving.address)),
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
async fn with_the_pool_full_a_connection_is_closed_unread() {
    let mut serving = serve(0).await;
    let mut stream = tokio::net::TcpStream::connect(serving.address)
        .await
        .unwrap();
    // Closed before any upgrade answer: the read ends at once.
    let mut rest = Vec::new();
    tokio::time::timeout(
        REAL,
        tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut rest),
    )
    .await
    .expect("closed in time")
    .ok();
    assert!(rest.is_empty(), "nothing was answered: {rest:?}");
    assert!(serving.joined.try_recv().is_err());
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
