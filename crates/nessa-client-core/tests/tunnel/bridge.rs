//! Design rows T6 (either side of a bridge ending closes the other) and the
//! round trip both ends of a tunnel rely on.
use super::{bridge, dial, loopback_pair, websocket_config};
use nessa_protocol::tunnel::{TunnelAddress, TunnelSecret};
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

struct HeldSecret([u8; 32]);

impl nessa_auth::application::pairing::ClientPendingStore for HeldSecret {
    fn load_pending(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::PendingEnrollment>,
        nessa_auth::application::pairing::PrivateStateError,
    > {
        Ok(None)
    }
    fn load_credential(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::DeviceCredential>,
        nessa_auth::application::pairing::PrivateStateError,
    > {
        Ok(None)
    }
    fn save_credential(
        &self,
        _: &nessa_auth::domain::CredentialId,
        _: &nessa_auth::domain::ResourceId,
        _: nessa_auth::domain::pairing::PublicIntent,
    ) -> Result<(), nessa_auth::application::pairing::PrivateStateError> {
        Ok(())
    }
    fn end_enrollment(
        &self,
        _: nessa_auth::domain::pairing::PublicIntent,
    ) -> Result<(), nessa_auth::application::pairing::PrivateStateError> {
        Ok(())
    }
    fn save_pending(
        &self,
        _: &nessa_auth::application::pairing::PrivateKeyMaterial,
        _: &[u8; 44],
        _: nessa_auth::domain::pairing::PublicIntent,
        _: Option<nessa_auth::domain::pairing::PublicIntent>,
    ) -> Result<(), nessa_auth::application::pairing::PrivateStateError> {
        Ok(())
    }
    fn load_tunnel_secret(
        &self,
    ) -> Result<Option<[u8; 32]>, nessa_auth::application::pairing::PrivateStateError> {
        Ok(Some(self.0))
    }
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
    let secret = TunnelSecret::new([1; 32]);
    let mut socket =
        tokio::task::spawn_blocking(move || dial(&address, &secret, Duration::from_secs(5)))
            .await
            .unwrap()
            .unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
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
        let secret = TunnelSecret::new([1; 32]);
        let mut socket = dial(&address, &secret, Duration::from_secs(5))?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.read(&mut [0u8; 16])
    })
    .await
    .unwrap();
    assert_eq!(
        read.unwrap(),
        0,
        "the gateway's close reaches the reader as end of stream"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tunnel_that_does_not_answer_is_refused_within_its_budget() {
    // A listener that never upgrades.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let address = TunnelAddress::try_from(format!("http://127.0.0.1:{port}")).unwrap();
    let started = std::time::Instant::now();
    let secret = TunnelSecret::new([1; 32]);
    let dialed =
        tokio::task::spawn_blocking(move || dial(&address, &secret, Duration::from_millis(300)))
            .await;
    assert!(dialed.unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(listener);
}

/// A phone away from the gateway's network: its product connection's direct
/// dial is refused, and the gateway's tunnel carries it instead. With no
/// tunnel the refusal stands.
#[tokio::test(flavor = "multi_thread")]
async fn a_product_connection_falls_back_on_the_gateways_tunnel() {
    use crate::product::{LocalProductConnector, ProductConnector};
    let tunnel = edge(echo).await;
    // A port nothing listens on: the direct dial is refused at once.
    let direct = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let alone = tokio::task::spawn_blocking(move || {
        LocalProductConnector::default()
            .connect(direct, Duration::from_secs(5))
            .err()
            .map(|error| error.kind())
    })
    .await
    .unwrap();
    // With no tunnel the plain direct dial's own refusal, as before.
    assert_eq!(alone, Some(std::io::ErrorKind::ConnectionRefused));
    let echoed = tokio::task::spawn_blocking(move || {
        let mut connected = LocalProductConnector::reaching(
            Some(tunnel),
            Some(std::sync::Arc::new(HeldSecret([3; 32]))),
            std::sync::Arc::new(std::sync::Mutex::new(None)),
        )
        .connect(direct, Duration::from_secs(5))
        .unwrap();
        connected
            .stream
            .write_timeout(Duration::from_secs(5))
            .unwrap();
        connected
            .stream
            .read_timeout(Duration::from_secs(5))
            .unwrap();
        connected.stream.write_all(b"native hello").unwrap();
        let mut received = [0u8; 12];
        connected.stream.read_exact(&mut received).unwrap();
        connected.interrupt.interrupt();
        received
    })
    .await
    .unwrap();
    assert_eq!(&echoed, b"native hello");
}

/// The paired gateway's key, and the credential that pins it.
struct Paired {
    pin: [u8; 44],
    secret: [u8; 32],
}

impl nessa_auth::application::pairing::ClientPendingStore for Paired {
    fn load_pending(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::PendingEnrollment>,
        nessa_auth::application::pairing::PrivateStateError,
    > {
        Ok(None)
    }
    fn load_credential(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::DeviceCredential>,
        nessa_auth::application::pairing::PrivateStateError,
    > {
        use nessa_auth::application::pairing::{
            DeviceCredential, PendingEnrollment, PrivateKeyMaterial,
        };
        use nessa_auth::domain::{
            pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent},
            CredentialId, ResourceId,
        };
        let intent = PublicIntent::new(
            InvitationId::new([1; InvitationId::LENGTH]),
            AttemptId::new([2; AttemptId::LENGTH]),
            ConsentIntentId::new([3; ConsentIntentId::LENGTH]),
            1,
            9,
            ConsentClass::DeviceRead,
        )
        .unwrap();
        let pending = PendingEnrollment::new(
            PrivateKeyMaterial::new(zeroize::Zeroizing::new([5; 32])),
            self.pin,
            intent,
        );
        Ok(Some(DeviceCredential::new(
            pending,
            CredentialId::new("device-1").unwrap(),
            ResourceId::new("receiver-1").unwrap(),
        )))
    }
    fn save_credential(
        &self,
        _: &nessa_auth::domain::CredentialId,
        _: &nessa_auth::domain::ResourceId,
        _: nessa_auth::domain::pairing::PublicIntent,
    ) -> Result<(), nessa_auth::application::pairing::PrivateStateError> {
        Ok(())
    }
    fn end_enrollment(
        &self,
        _: nessa_auth::domain::pairing::PublicIntent,
    ) -> Result<(), nessa_auth::application::pairing::PrivateStateError> {
        Ok(())
    }
    fn save_pending(
        &self,
        _: &nessa_auth::application::pairing::PrivateKeyMaterial,
        _: &[u8; 44],
        _: nessa_auth::domain::pairing::PublicIntent,
        _: Option<nessa_auth::domain::pairing::PublicIntent>,
    ) -> Result<(), nessa_auth::application::pairing::PrivateStateError> {
        Ok(())
    }
    fn load_tunnel_secret(
        &self,
    ) -> Result<Option<[u8; 32]>, nessa_auth::application::pairing::PrivateStateError> {
        Ok(Some(self.secret))
    }
}

/// A relay that answers one lookup with `origin` and does not check the seal.
fn scripted_relay(
    origin: nessa_protocol::relay::PublishedOrigin,
) -> nessa_protocol::relay::RelayAddress {
    use nessa_protocol::relay::{read_frame, write_frame, RelayFrame};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if read_frame(&mut stream).is_err() {
                continue;
            }
            if write_frame(&mut stream, &RelayFrame::Challenge { nonce: [3; 32] }).is_err() {
                continue;
            }
            if read_frame(&mut stream).is_err() {
                continue;
            }
            let _ = write_frame(
                &mut stream,
                &RelayFrame::Located {
                    origin: Box::new(origin),
                },
            );
        }
    });
    nessa_protocol::relay::RelayAddress::try_from(format!("127.0.0.1:{port}")).unwrap()
}

fn gateway() -> nessa_auth::adapters::pairing::NativeIdentity {
    use nessa_auth::adapters::pairing::NativeIdentity;
    use nessa_auth::application::pairing::PrivateKeyMaterial;
    NativeIdentity::restore(PrivateKeyMaterial::new(zeroize::Zeroizing::new([4; 32]))).unwrap()
}

/// A listener whose backlog is full, so another connect waits out its timeout.
fn hung_lan() -> (socket2::Socket, std::net::SocketAddr, Vec<TcpStream>) {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .unwrap();
    socket
        .bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into())
        .unwrap();
    socket.listen(1).unwrap();
    let address = socket.local_addr().unwrap().as_socket().unwrap();
    let mut held = Vec::new();
    for _ in 0..8 {
        match TcpStream::connect_timeout(&address, Duration::from_millis(50)) {
            Ok(stream) => held.push(stream),
            Err(_) => break,
        }
    }
    let started = std::time::Instant::now();
    let extra = TcpStream::connect_timeout(&address, Duration::from_millis(200));
    assert!(
        extra.is_err(),
        "the lan dial did not hang; {} connections were held",
        held.len()
    );
    assert!(started.elapsed() >= Duration::from_millis(150));
    (socket, address, held)
}

/// The relay names an attacker's origin. The phone must not dial it, so the
/// reusable tunnel secret never leaves for that peer.
#[tokio::test(flavor = "multi_thread")]
async fn a_relay_that_names_an_attackers_origin_is_never_sent_the_secret() {
    use crate::product::{LocalProductConnector, ProductConnector};
    use nessa_protocol::relay::PublishedOrigin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    let hits = Arc::new(AtomicUsize::new(0));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let counted = hits.clone();
    std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < until {
            match listener.accept() {
                Ok(_) => {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    let origin = format!("http://127.0.0.1:{port}");
    let forged = PublishedOrigin::signed(&[0; 64], &origin).unwrap();
    let relay = scripted_relay(forged);
    let identity = gateway();
    let store = Arc::new(Paired {
        pin: identity.public_spki(),
        secret: [9; 32],
    });
    let direct = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let connected = tokio::task::spawn_blocking(move || {
        LocalProductConnector::reaching(None, Some(store), Arc::new(Mutex::new(Some(relay))))
            .connect(direct, Duration::from_secs(2))
    })
    .await
    .unwrap();
    assert!(connected.is_err(), "a forged origin was dialed");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the attacker's origin was dialed"
    );
}

/// The direct dial hangs and the saved tunnel address is closed. Discovery
/// still reaches the signed origin inside the one connect budget.
#[tokio::test(flavor = "multi_thread")]
async fn a_hung_lan_dial_and_a_stale_address_still_connect_in_time() {
    use crate::product::{LocalProductConnector, ProductConnector};
    use nessa_protocol::relay::PublishedOrigin;
    use std::sync::{Arc, Mutex};

    let live = edge(echo).await;
    let identity = gateway();
    let signature = identity
        .sign_tunnel_origin(live.as_str().as_bytes())
        .unwrap();
    let signed = PublishedOrigin::signed(&signature, live.as_str()).unwrap();
    let relay = scripted_relay(signed);
    let stale_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let stale = TunnelAddress::try_from(format!("http://127.0.0.1:{stale_port}")).unwrap();
    let (hung, lan, held) = tokio::task::spawn_blocking(hung_lan).await.unwrap();
    let store = Arc::new(Paired {
        pin: identity.public_spki(),
        secret: [7; 32],
    });
    let started = std::time::Instant::now();
    let connected = tokio::task::spawn_blocking(move || {
        LocalProductConnector::reaching(Some(stale), Some(store), Arc::new(Mutex::new(Some(relay))))
            .connect(lan, Duration::from_secs(5))
    })
    .await
    .unwrap();
    let elapsed = started.elapsed();
    let mut connected = connected.expect("the published origin was not reached in time");
    assert!(
        elapsed < Duration::from_secs(3),
        "recovery waited out the hung dial: {elapsed:?}"
    );
    connected
        .stream
        .write_timeout(Duration::from_secs(2))
        .unwrap();
    connected
        .stream
        .read_timeout(Duration::from_secs(2))
        .unwrap();
    connected.stream.write_all(b"ping").unwrap();
    let mut received = [0; 4];
    connected.stream.read_exact(&mut received).unwrap();
    assert_eq!(&received, b"ping");
    drop(connected);
    drop(hung);
    drop(held);
}
