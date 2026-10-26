//! Design rows T6 (either side of a bridge ending closes the other) and the
//! round trip both ends of a tunnel rely on.
use super::{bridge, dial, loopback_pair, websocket_config};
use nessa_protocol::tunnel::TunnelAddress;
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};
use tokio::net::TcpListener;

/// An edge on loopback that bridges each WebSocket on `/native` to a
/// loopback pair, and hands the far end of each pair to `serve`.
async fn edge(serve: fn(TcpStream)) -> TunnelAddress {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let websocket =
                    tokio_tungstenite::accept_async_with_config(stream, Some(websocket_config()))
                        .await
                        .unwrap();
                let (ours, theirs) = loopback_pair(Duration::from_secs(5)).await.unwrap();
                std::thread::spawn(move || serve(theirs));
                bridge(websocket, ours).await;
            });
        }
    });
    TunnelAddress::try_from(format!("http://127.0.0.1:{port}")).unwrap()
}

fn echo(mut socket: TcpStream) {
    let mut buffer = [0u8; 4096];
    loop {
        match socket.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                if socket.write_all(&buffer[..count]).is_err() {
                    return;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn bytes_cross_a_tunnel_both_ways_unchanged() {
    let address = edge(echo).await;
    let mut socket = tokio::task::spawn_blocking(move || dial(&address, Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // Larger than one bridge chunk, so it crosses as several messages.
    let sent: Vec<u8> = (0..100_000u32).map(|index| (index % 251) as u8).collect();
    let mut received = vec![0u8; sent.len()];
    let writer = {
        let mut socket = socket.try_clone().unwrap();
        let sent = sent.clone();
        std::thread::spawn(move || socket.write_all(&sent).unwrap())
    };
    tokio::task::spawn_blocking(move || {
        socket.read_exact(&mut received).unwrap();
        received
    })
    .await
    .map(|received| assert_eq!(received, sent))
    .unwrap();
    writer.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_far_end_closing_closes_the_reader() {
    let address = edge(drop).await;
    let read = tokio::task::spawn_blocking(move || {
        let mut socket = dial(&address, Duration::from_secs(5))?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.read(&mut [0u8; 16])
    })
    .await
    .unwrap();
    assert_eq!(read.unwrap(), 0, "the gateway's close reaches the reader as end of stream");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_that_does_not_answer_is_refused_within_its_budget() {
    // A listener that never upgrades.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let address = TunnelAddress::try_from(format!("http://127.0.0.1:{port}")).unwrap();
    let started = std::time::Instant::now();
    let dialed =
        tokio::task::spawn_blocking(move || dial(&address, Duration::from_millis(300))).await;
    assert!(dialed.unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(listener);
}
