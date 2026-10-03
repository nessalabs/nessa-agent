//! Crypto fixture messages use raw TLS; lengths travel through fixture-only ports.
//! No application prefix, parser, frame ceiling or product phase is implemented here.
use super::super::{GatewayTrust, NativeIdentity, NativeTransport};
#[cfg(unix)]
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::ops::Deref;
#[cfg(unix)]
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
pub(crate) struct MessageLengths {
    send: Sender<usize>,
    receive: Receiver<usize>,
}
#[cfg(unix)]
pub(crate) fn message_lengths() -> (MessageLengths, MessageLengths) {
    let (a, at_b) = mpsc::channel();
    let (b, at_a) = mpsc::channel();
    (
        MessageLengths {
            send: a,
            receive: at_a,
        },
        MessageLengths {
            send: b,
            receive: at_b,
        },
    )
}
#[cfg(unix)]
pub(crate) struct CryptoFixtureTransport<S: Read + Write> {
    transport: NativeTransport<S>,
    lengths: MessageLengths,
}
#[cfg(unix)]
impl<S: Read + Write> CryptoFixtureTransport<S> {
    pub(crate) fn new(transport: NativeTransport<S>, lengths: MessageLengths) -> Self {
        Self { transport, lengths }
    }
    pub(crate) fn read_message(&mut self) -> io::Result<Vec<u8>> {
        let size = self
            .lengths
            .receive
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let mut bytes = vec![0; size];
        self.transport.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    pub(crate) fn write_message(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.lengths
            .send
            .send(bytes.len())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        self.transport.write_all(bytes)?;
        self.transport.flush()
    }
    pub(crate) fn into_transport(self) -> NativeTransport<S> {
        self.transport
    }
}
#[cfg(unix)]
impl<S: Read + Write> Deref for CryptoFixtureTransport<S> {
    type Target = NativeTransport<S>;
    fn deref(&self) -> &Self::Target {
        &self.transport
    }
}

/// Actual public transport contexts for crypto fixtures on every supported platform.
/// Both sockets have finite physical IO deadlines; the server thread is joined
/// before either TLS result is unwrapped. This is not a native listener fixture.
pub(crate) fn native_channels(
    gateway: &NativeIdentity,
    device: &NativeIdentity,
) -> (NativeTransport<TcpStream>, NativeTransport<TcpStream>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    for stream in [&client, &server] {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
    }
    thread::scope(|scope| {
        let accept = scope.spawn(|| NativeTransport::accept(server, gateway));
        let connected =
            NativeTransport::connect(client, device, GatewayTrust::Pinned(gateway.public_spki()));
        let accepted = accept.join();
        (accepted.unwrap().unwrap(), connected.unwrap())
    })
}
