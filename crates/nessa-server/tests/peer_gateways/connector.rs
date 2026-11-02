use super::*;
use nessa_protocol::relay::DIRECT_HEAD_START;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

const KEY: GatewayKey = [5; 32];

/// A connected stream that is not the relay's: what a direct connect gives.
fn direct_stream() -> (TcpStream, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    (stream, listener)
}

/// A relay that takes one `Connect{KEY}` and answers `answer`, then `after`.
async fn scripted_relay(
    answer: RelayFrame,
    after: &'static [u8],
) -> (RelayAddress, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap();
    let serving = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(matches!(
            read_frame(&mut stream).await.unwrap(),
            RelayFrame::Connect { key } if key == KEY
        ));
        let mut bytes = answer.encode();
        bytes.extend_from_slice(after);
        stream.write_all(&bytes).await.unwrap();
        // Hold the connection until the reader is done with it.
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut [0; 1]).await;
    });
    (address, serving)
}

#[tokio::test]
async fn no_relay_is_the_direct_connect_alone() {
    let (stream, _listener) = direct_stream();
    let local = stream.local_addr().unwrap();
    let used = raced(
        async { Ok(stream) },
        None::<PeerConnectFuture>,
        tokio::time::sleep(DIRECT_HEAD_START),
        &Mutex::new(None),
    )
    .await
    .unwrap();
    assert_eq!(used.local_addr().unwrap(), local);
}

#[tokio::test]
async fn a_direct_connect_first_never_dials_the_relay() {
    let relay = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    relay.set_nonblocking(true).unwrap();
    let address = RelayAddress::try_from(relay.local_addr().unwrap().to_string()).unwrap();
    let (stream, _listener) = direct_stream();
    let local = stream.local_addr().unwrap();
    let used = raced(
        async { Ok(stream) },
        Some(TcpPeerConnector.relay(&address, KEY)),
        tokio::time::sleep(DIRECT_HEAD_START),
        &Mutex::new(None),
    )
    .await
    .unwrap();
    assert_eq!(used.local_addr().unwrap(), local);
    assert_eq!(
        relay.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "the relay was never dialed"
    );
}

#[tokio::test]
async fn a_refused_direct_connect_reads_through_the_relay_at_once() {
    let (address, serving) = scripted_relay(RelayFrame::Joined, b"tls").await;
    let started = Instant::now();
    let mut joined = raced(
        async { Err(io::ErrorKind::ConnectionRefused.into()) },
        Some(TcpPeerConnector.relay(&address, KEY)),
        tokio::time::sleep(DIRECT_HEAD_START),
        &Mutex::new(None),
    )
    .await
    .unwrap();
    assert!(
        started.elapsed() < DIRECT_HEAD_START,
        "no head start waited"
    );
    let mut first = [0; 3];
    joined.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"tls");
    drop(joined);
    serving.await.unwrap();
}

#[tokio::test]
async fn a_direct_connect_still_waiting_after_the_head_start_is_raced_by_the_relay() {
    let (address, serving) = scripted_relay(RelayFrame::Joined, b"tls").await;
    let mut joined = raced(
        std::future::pending(),
        Some(TcpPeerConnector.relay(&address, KEY)),
        tokio::time::sleep(DIRECT_HEAD_START),
        &Mutex::new(None),
    )
    .await
    .unwrap();
    let mut first = [0; 3];
    joined.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"tls");
    drop(joined);
    serving.await.unwrap();
}

#[tokio::test]
async fn a_relay_that_refuses_first_leaves_a_slow_direct_connect_to_connect() {
    let (address, serving) = scripted_relay(RelayFrame::Offline, b"").await;
    let (stream, _listener) = direct_stream();
    let local = stream.local_addr().unwrap();
    let slow = async {
        // Past the head start, and so past the relay's refusal.
        tokio::time::sleep(DIRECT_HEAD_START + Duration::from_millis(500)).await;
        Ok(stream)
    };
    let used = raced(
        slow,
        Some(TcpPeerConnector.relay(&address, KEY)),
        tokio::time::sleep(DIRECT_HEAD_START),
        &Mutex::new(None),
    )
    .await
    .unwrap();
    assert_eq!(used.local_addr().unwrap(), local);
    serving.await.unwrap();
}

#[tokio::test]
async fn both_failing_is_the_relays_refusal() {
    for answer in [RelayFrame::Offline, RelayFrame::NoAnswer] {
        let (address, serving) = scripted_relay(answer.clone(), b"").await;
        let refused = raced(
            async { Err(io::ErrorKind::TimedOut.into()) },
            Some(TcpPeerConnector.relay(&address, KEY)),
            tokio::time::sleep(DIRECT_HEAD_START),
            &Mutex::new(None),
        )
        .await
        .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(
            RelayRefusal::carried(&refused),
            RelayRefusal::of(answer),
            "the poller tells away from unreachable by it"
        );
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn a_relay_answering_a_gateways_frame_is_invalid() {
    let (address, serving) = scripted_relay(RelayFrame::Registered, b"").await;
    let refused = raced(
        async { Err(io::ErrorKind::ConnectionRefused.into()) },
        Some(TcpPeerConnector.relay(&address, KEY)),
        tokio::time::sleep(DIRECT_HEAD_START),
        &Mutex::new(None),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
    serving.await.unwrap();
}

#[tokio::test]
async fn a_relay_refusal_outlives_a_direct_connect_dropped_at_the_callers_deadline() {
    let (address, serving) = scripted_relay(RelayFrame::Offline, b"").await;
    let refused = Mutex::new(None);
    let dropped = tokio::time::timeout(
        DIRECT_HEAD_START + Duration::from_millis(500),
        raced(
            std::future::pending(),
            Some(TcpPeerConnector.relay(&address, KEY)),
            tokio::time::sleep(DIRECT_HEAD_START),
            &refused,
        ),
    )
    .await;
    assert!(dropped.is_err(), "the direct connect still waits");
    let refused = refused.into_inner().unwrap().unwrap();
    assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
    serving.await.unwrap();
}
