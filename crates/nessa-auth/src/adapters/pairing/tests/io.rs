//! Crypto fixture messages use raw TLS; lengths travel through fixture-only ports.
//! No application prefix, parser, frame ceiling or product phase is implemented here.
use super::super::NativeTransport;
use std::io::{self, Read, Write};
use std::ops::Deref;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

pub(crate) struct MessageLengths {
    send: Sender<usize>,
    receive: Receiver<usize>,
}
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
pub(crate) struct CryptoFixtureTransport<S: Read + Write> {
    transport: NativeTransport<S>,
    lengths: MessageLengths,
}
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
impl<S: Read + Write> Deref for CryptoFixtureTransport<S> {
    type Target = NativeTransport<S>;
    fn deref(&self) -> &Self::Target {
        &self.transport
    }
}
