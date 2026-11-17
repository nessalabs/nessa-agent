use super::*;
use crate::{relay::application::RelayLimits, timer::TokioTimer};
use nessa_auth::{adapters::pairing::NativeIdentity, application::pairing::PrivateKeyMaterial};
use nessa_protocol::relay::{read_frame, write_frame, RelayFrame};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream as StdStream},
    time::Duration,
};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

fn dial(address: SocketAddr) -> StdStream {
    let stream = StdStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_and_a_reader_on_blocking_sockets_are_joined_over_tcp_and_stop_closes_them() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
    let relay = Relay::new(
        timer.clone(),
        Arc::new(OsRelayEntropy),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    let (stop, stopping) = oneshot::channel::<()>();
    let serving = tokio::spawn(serve_tcp(listener, relay, timer, async {
        let _ = stopping.await;
    }));

    let (registered, is_registered) = std::sync::mpsc::channel();
    let gateway = std::thread::spawn(move || {
        let identity =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([4; 32]))).unwrap();
        let key = identity.public_key();
        let mut serve = dial(address);
        write_frame(&mut serve, &RelayFrame::Serve { key }).unwrap();
        let RelayFrame::Challenge { nonce } = read_frame(&mut serve).unwrap() else {
            panic!("no challenge");
        };
        let signature = identity.sign_relay_challenge(&nonce).unwrap();
        write_frame(&mut serve, &RelayFrame::Proof { signature }).unwrap();
        assert_eq!(read_frame(&mut serve).unwrap(), RelayFrame::Registered);
        registered.send(key).unwrap();
        let RelayFrame::Incoming { token } = read_frame(&mut serve).unwrap() else {
            panic!("no incoming reader");
        };
        let mut accepted = dial(address);
        write_frame(&mut accepted, &RelayFrame::Accept { key, token }).unwrap();
        assert_eq!(read_frame(&mut accepted).unwrap(), RelayFrame::Joined);
        let mut hello = [0; 5];
        accepted.read_exact(&mut hello).unwrap();
        assert_eq!(&hello, b"hello");
        accepted.write_all(b"world").unwrap();
        (serve, accepted)
    });

    let key = is_registered.recv_timeout(Duration::from_secs(5)).unwrap();
    let reader = std::thread::spawn(move || {
        let mut reader = dial(address);
        write_frame(&mut reader, &RelayFrame::Connect { key }).unwrap();
        assert_eq!(read_frame(&mut reader).unwrap(), RelayFrame::Joined);
        reader.write_all(b"hello").unwrap();
        let mut world = [0; 5];
        reader.read_exact(&mut world).unwrap();
        assert_eq!(&world, b"world");
        reader
    });
    let mut reader = tokio::task::spawn_blocking(move || reader.join().unwrap())
        .await
        .unwrap();
    let (mut serve, _accepted) = tokio::task::spawn_blocking(move || gateway.join().unwrap())
        .await
        .unwrap();

    // Stopping the relay closes everything it held.
    stop.send(()).unwrap();
    serving.await.unwrap();
    let ended = tokio::task::spawn_blocking(move || {
        let mut rest = Vec::new();
        (
            reader.read_to_end(&mut rest).map(|_| rest.len()),
            serve.read_to_end(&mut Vec::new()).map(|_| ()),
        )
    })
    .await
    .unwrap();
    assert_eq!(ended.0.unwrap(), 0);
    ended.1.unwrap();
}
