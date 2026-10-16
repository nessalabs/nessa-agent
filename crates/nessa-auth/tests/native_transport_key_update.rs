//! Actual TLS control records preserve permanent possession and application bytes.
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport},
    application::pairing::PrivateKeyMaterial,
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use zeroize::Zeroizing;
fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}
fn bounded(socket: TcpStream) -> TcpStream {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
}
struct Counted {
    socket: TcpStream,
    written: Arc<AtomicUsize>,
}
impl Read for Counted {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.socket.read(bytes)
    }
}
impl Write for Counted {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.socket.write(bytes)?;
        self.written.fetch_add(written, Ordering::SeqCst);
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
fn counted(socket: TcpStream) -> (Counted, Arc<AtomicUsize>) {
    let written = Arc::new(AtomicUsize::new(0));
    (
        Counted {
            socket: bounded(socket),
            written: written.clone(),
        },
        written,
    )
}
#[test]
fn native_tls_key_update_preserves_pinned_identity_proof_and_application_stream() {
    let gateway = identity(10);
    let pin = gateway.public_spki();
    let device = identity(20);
    let key = device.device_key();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let (socket, written) = counted(socket);
        let mut transport = NativeTransport::accept(socket, &gateway).unwrap();
        assert_eq!(transport.device_proof().key(), key);
        let before = written.load(Ordering::SeqCst);
        transport.request_key_update().unwrap();
        assert!(
            written.load(Ordering::SeqCst) > before,
            "key update must flush control bytes before application writes"
        );
        transport.write_all(b"actual application").unwrap();
        transport.flush().unwrap();
        let mut response = [0; 2];
        transport.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"ok");
        assert_eq!(transport.device_proof().key(), key);
        assert_eq!(transport.gateway_spki(), pin);
    });
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
    let (socket, written) = counted(socket);
    let mut transport =
        NativeTransport::connect(socket, &device, GatewayTrust::Pinned(pin)).unwrap();
    let before = written.load(Ordering::SeqCst);
    transport.request_key_update().unwrap();
    assert!(
        written.load(Ordering::SeqCst) > before,
        "key update must flush control bytes before application writes"
    );
    let mut body = [0; 18];
    transport.read_exact(&mut body).unwrap();
    assert_eq!(&body, b"actual application");
    assert_eq!(transport.device_proof().key(), key);
    assert_eq!(transport.gateway_spki(), pin);
    transport.write_all(b"ok").unwrap();
    transport.flush().unwrap();
    server.join().unwrap();
}
