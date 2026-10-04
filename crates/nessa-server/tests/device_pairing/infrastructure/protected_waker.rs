//! Review finding F1: the reading task must never take the writing task's
//! socket wakeup. A socket keeps one waker per direction; here a small duplex
//! pipe, which behaves the same way, stands in for the socket, with a relay to
//! the real TLS peer whose forwarding toward the peer is held by a gate.
use super::super::connection::{wake::WakeEndpoint, DeadlineStream};
use super::super::frames::{encode_frame, FrameReader};
use super::*;
use crate::app::dependencies::RuntimeDependencies;
use futures_util::{SinkExt, StreamExt};
use nessa_auth::adapters::pairing::{GatewayTrust, NativeIdentity, OsEntropy};
use std::task::{Context, Poll};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};

const RESPONSE: usize = 100_000;

/// The writer is parked on a full socket; the reader then runs, receiving a
/// request; when the socket drains, the writer must be the one woken and its
/// frame must complete. Before the fix the reader also polled the write side,
/// so it was woken in the writer's place and the send never finished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_never_takes_the_writers_wakeup() {
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request, requested) = mpsc::channel::<()>();
    let peer = std::thread::spawn(move || {
        let socket = std::net::TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let mut tls = NativeTransport::connect(socket, &device, GatewayTrust::Pinned(pin)).unwrap();
        requested.recv().unwrap();
        tls.write_all(&encode_frame(MAX_PROTECTED_REQUEST_BYTES, b"request").unwrap())
            .unwrap();
        tls.flush().unwrap();
        let mut frames = FrameReader::new(MAX_PROTECTED_RESPONSE_BYTES);
        loop {
            let buffer = frames.unfilled().unwrap();
            let count = tls.read(buffer).unwrap();
            assert!(count > 0, "the gateway closed before the response");
            if let Some(body) = frames.filled(count).unwrap() {
                return body.len();
            }
        }
    });
    let (accepted, peer_address) = listener.accept().unwrap();
    let transport = tokio::task::spawn_blocking(move || {
        let (stream, _) = DeadlineStream::new(
            accepted,
            RuntimeDependencies::default().clock,
            WakeEndpoint::new(peer_address),
        )
        .unwrap();
        stream.blocking().unwrap();
        NativeTransport::accept(stream, &gateway).unwrap()
    })
    .await
    .unwrap();
    let mut transport = transport;
    let upstream = transport.stream_mut().take_socket().unwrap();
    upstream.set_nonblocking(true).unwrap();
    let upstream = TcpStream::from_std(upstream).unwrap();
    // A 1 KiB pipe in place of the socket; the relay forwards to the peer
    // only once the gate opens.
    let (socket, relay) = tokio::io::duplex(1024);
    let (gate, opened) = tokio::sync::oneshot::channel::<()>();
    let (mut relay_read, mut relay_write) = tokio::io::split(relay);
    let (mut upstream_read, mut upstream_write) = upstream.into_split();
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut upstream_read, &mut relay_write).await;
    });
    tokio::spawn(async move {
        let _ = opened.await;
        let mut buffer = [0; 4096];
        loop {
            match relay_read.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if upstream_write.write_all(&buffer[..count]).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let connection = ProtectedConnection::over(transport, socket);
    let (mut sink, mut stream) = connection.split();
    let writer = tokio::spawn(async move { sink.send(vec![b'x'; RESPONSE]).await });
    // The writer fills the pipe and parks on the write side.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!writer.is_finished());
    let reader = tokio::spawn(async move {
        let first = stream.next().await;
        // Keep reading, as the session loop does.
        let _ = stream.next().await;
        first
    });
    request.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    gate.send(()).unwrap();
    let written = tokio::time::timeout(Duration::from_secs(10), writer)
        .await
        .expect("the parked writer is woken when the socket drains");
    assert!(written.unwrap().is_ok());
    assert_eq!(peer.join().unwrap(), RESPONSE);
    reader.abort();
}

/// A socket double: it delivers `incoming` and records every byte written.
struct Recorder {
    incoming: Vec<u8>,
    written: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}
impl tokio::io::AsyncRead for Recorder {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let count = buffer.remaining().min(this.incoming.len());
        buffer.put_slice(&this.incoming[..count]);
        this.incoming.drain(..count);
        if count == 0 {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }
}
impl tokio::io::AsyncWrite for Recorder {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.written.lock().unwrap().extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Review round 2, L2: a record that fails TLS authentication ends the read
/// with an error and TLS queues a fatal alert. No write follows, but dropping
/// the connection sends the alert before the socket closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fatal_alert_is_sent_before_the_close() {
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let socket = std::net::TcpStream::connect(address).unwrap();
        let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
        NativeTransport::connect(socket, &device, GatewayTrust::Pinned(pin)).map(drop)
    });
    let (accepted, peer_address) = listener.accept().unwrap();
    let mut transport = tokio::task::spawn_blocking(move || {
        let (stream, _) = DeadlineStream::new(
            accepted,
            RuntimeDependencies::default().clock,
            WakeEndpoint::new(peer_address),
        )
        .unwrap();
        stream.blocking().unwrap();
        NativeTransport::accept(stream, &gateway).unwrap()
    })
    .await
    .unwrap();
    peer.join().unwrap().unwrap();
    let _socket = transport.stream_mut().take_socket().unwrap();
    // An application-data record whose contents cannot be authenticated.
    let mut forged = vec![0x17, 0x03, 0x03, 0x00, 0x20];
    forged.extend_from_slice(&[0x5a; 0x20]);
    let written = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut connection = ProtectedConnection::over(
        transport,
        Recorder {
            incoming: forged,
            written: written.clone(),
        },
    );
    let read = connection.next().await;
    assert!(
        matches!(read, Some(Err(_))),
        "the forged record fails the read"
    );
    assert!(written.lock().unwrap().is_empty(), "reading writes nothing");
    drop(connection);
    let sent = written.lock().unwrap().clone();
    assert!(
        !sent.is_empty(),
        "the queued fatal alert reaches the socket"
    );
    // An encrypted TLS 1.3 alert travels as an application-data record.
    assert_eq!(&sent[..3], &[0x17, 0x03, 0x03]);
}
