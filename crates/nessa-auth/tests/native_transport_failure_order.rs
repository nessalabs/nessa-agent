//! Canonical TLS failure is visible while its original physical cleanup is held.
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport, PairingCryptoError},
    application::pairing::PrivateKeyMaterial,
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};
use zeroize::Zeroizing;
const WAIT: Duration = Duration::from_secs(5);
fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}
fn bounded(socket: TcpStream) -> TcpStream {
    socket.set_read_timeout(Some(WAIT)).unwrap();
    socket.set_write_timeout(Some(WAIT)).unwrap();
    socket
}
struct HeldDrop {
    socket: TcpStream,
    dropping: Sender<()>,
    release: Receiver<()>,
}
impl Read for HeldDrop {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.read(bytes)
    }
}
impl Write for HeldDrop {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
impl Drop for HeldDrop {
    fn drop(&mut self) {
        self.dropping.send(()).unwrap();
        // Disconnection releases on assertion unwind as well as normal cleanup.
        let _ = self.release.recv_timeout(WAIT);
    }
}
fn failure_before_cleanup(pin: [u8; 44], expected: PairingCryptoError, handshakes: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        if handshakes {
            assert!(NativeTransport::accept(bounded(socket), &identity(10)).is_err());
        }
    });
    let (dropping, dropped) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (observed, observation) = mpsc::channel();
    let (finished, completion) = mpsc::channel();
    let worker = thread::spawn(move || {
        let socket = bounded(TcpStream::connect_timeout(&address, WAIT).unwrap());
        let result = NativeTransport::connect_observed(
            HeldDrop {
                socket,
                dropping,
                release: released,
            },
            &identity(20),
            GatewayTrust::Pinned(pin),
            |failure| observed.send(failure).unwrap(),
        );
        finished.send(result.err().unwrap()).unwrap();
    });
    dropped.recv_timeout(WAIT).unwrap();
    assert_eq!(
        observation.try_recv().unwrap(),
        expected,
        "failure must be observed before physical stream Drop waits"
    );
    assert!(
        completion.try_recv().is_err(),
        "observing failure does not report physical cleanup complete"
    );
    release.send(()).unwrap();
    assert_eq!(completion.recv_timeout(WAIT).unwrap(), expected);
    worker.join().unwrap();
    server.join().unwrap();
    assert!(
        observation.try_recv().is_err(),
        "one canonical failure observation"
    );
}
#[test]
fn mismatched_real_tls_pin_is_observed_before_held_physical_drop() {
    failure_before_cleanup(
        identity(30).public_spki(),
        PairingCryptoError::InvalidProof,
        true,
    );
}
#[test]
fn malformed_pin_setup_failure_is_observed_before_held_physical_drop() {
    failure_before_cleanup([0; 44], PairingCryptoError::InvalidContext, false);
}
#[test]
fn successful_pinned_tls_emits_no_failure_observation() {
    let gateway = identity(10);
    let pin = gateway.public_spki();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut channel = NativeTransport::accept(bounded(socket), &gateway).unwrap();
        channel.write_all(b"ok").unwrap();
        channel.flush().unwrap();
    });
    let mut observations = 0;
    let socket = bounded(TcpStream::connect_timeout(&address, WAIT).unwrap());
    let mut channel =
        NativeTransport::connect_observed(socket, &identity(20), GatewayTrust::Pinned(pin), |_| {
            observations += 1
        })
        .unwrap();
    assert_eq!(observations, 0);
    let mut reply = [0; 2];
    channel.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"ok");
    assert_eq!(channel.gateway_spki(), pin);
    server.join().unwrap();
}
