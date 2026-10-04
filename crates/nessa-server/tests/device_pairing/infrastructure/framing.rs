use super::support::{sockets, WAIT};
use nessa_auth::{
    adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport, OsEntropy},
    domain::pairing::AttemptId,
};
use nessa_protocol::pairing::{
    wire::{decode_request, encode_refused, encode_request, NativePairingRequest, NativeWireError},
    EnrollmentChannel, NativeFrameError,
};
use std::{
    io::{ErrorKind, Read, Result as IoResult, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicIsize, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};

struct Controls {
    read: AtomicUsize,
    written: AtomicUsize,
    write_budget: AtomicIsize,
}
impl Controls {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            read: AtomicUsize::new(0),
            written: AtomicUsize::new(0),
            write_budget: AtomicIsize::new(-1),
        })
    }
}
struct ExternalIo {
    socket: TcpStream,
    control: Arc<Controls>,
}
impl Read for ExternalIo {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        let length = bytes.len().min(7);
        let count = self.socket.read(&mut bytes[..length])?;
        self.control.read.fetch_add(count, Ordering::SeqCst);
        Ok(count)
    }
}
impl Write for ExternalIo {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        let budget = self.control.write_budget.load(Ordering::SeqCst);
        if budget == 0 {
            return Err(ErrorKind::WouldBlock.into());
        }
        let length = if budget < 0 {
            bytes.len()
        } else {
            bytes.len().min(budget as usize)
        };
        let count = self.socket.write(&bytes[..length])?;
        if budget >= 0 {
            self.control
                .write_budget
                .fetch_sub(count as isize, Ordering::SeqCst);
        }
        self.control.written.fetch_add(count, Ordering::SeqCst);
        Ok(count)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.socket.flush()
    }
}
fn peers() -> (
    NativeTransport<ExternalIo>,
    NativeTransport<ExternalIo>,
    Arc<Controls>,
    Arc<Controls>,
) {
    let (a, b) = sockets();
    let server_control = Controls::new();
    let client_control = Controls::new();
    let gateway = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let pin = gateway.public_spki();
    let control = server_control.clone();
    let server = thread::spawn(move || {
        NativeTransport::accept(ExternalIo { socket: a, control }, &gateway).unwrap()
    });
    let client = NativeTransport::connect(
        ExternalIo {
            socket: b,
            control: client_control.clone(),
        },
        &device,
        GatewayTrust::Pinned(pin),
    )
    .unwrap();
    (
        server.join().unwrap(),
        client,
        server_control,
        client_control,
    )
}
fn partial(channel: &mut EnrollmentChannel<ExternalIo>, control: &Controls, target: usize) {
    let until = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < until, "actual partial IO did not arrive");
        assert_eq!(
            channel.receive_envelope(),
            Err(NativeFrameError::Io(ErrorKind::WouldBlock))
        );
        if control.read.load(Ordering::SeqCst) >= target {
            return;
        }
        thread::yield_now();
    }
}

#[test]
fn native_framing_retains_partial_io_and_output() {
    let (mut transport, mut peer, server_io, peer_io) = peers();
    transport.stream_mut().socket.set_nonblocking(true).unwrap();
    let read_start = server_io.read.load(Ordering::SeqCst);
    let written_start = peer_io.written.load(Ordering::SeqCst);
    let mut channel = EnrollmentChannel::new(transport);
    let bytes = encode_request(&NativePairingRequest::Hello(AttemptId::new(
        [4; AttemptId::LENGTH],
    )))
    .unwrap();
    let prefix = (bytes.len() as u32).to_be_bytes();
    peer.write_all(&prefix[..2]).unwrap();
    peer.flush().unwrap();
    partial(
        &mut channel,
        &server_io,
        read_start + peer_io.written.load(Ordering::SeqCst) - written_start,
    );
    peer.write_all(&prefix[2..]).unwrap();
    peer.write_all(&bytes[..11]).unwrap();
    peer.flush().unwrap();
    partial(
        &mut channel,
        &server_io,
        read_start + peer_io.written.load(Ordering::SeqCst) - written_start,
    );
    peer.write_all(&bytes[11..]).unwrap();
    peer.flush().unwrap();
    let until = Instant::now() + WAIT;
    let received = loop {
        assert!(Instant::now() < until);
        match channel.receive_envelope() {
            Err(NativeFrameError::Io(ErrorKind::WouldBlock)) => thread::yield_now(),
            result => break result.unwrap(),
        }
    };
    assert_eq!(received, bytes);
    assert!(matches!(
        decode_request(&received).unwrap(),
        NativePairingRequest::Hello(_)
    ));
    drop(channel);
    drop(peer);

    // Real TLS output accepts a partial physical write then WouldBlock. Resume
    // the same original envelope; replacement cannot overwrite queued bytes.
    let (server, mut client, _, client_io) = peers();
    // Non-blocking, like the first half: rustls may read while it writes, and a
    // blocking socket would hold that read until its timeout.
    client.stream_mut().socket.set_nonblocking(true).unwrap();
    let mut sender = EnrollmentChannel::new(client);
    let mut receiver = EnrollmentChannel::new(server);
    client_io.write_budget.store(3, Ordering::SeqCst);
    let payload = encode_refused().unwrap();
    assert_eq!(
        sender.send_envelope(&payload),
        Err(NativeFrameError::Io(ErrorKind::WouldBlock))
    );
    assert_eq!(
        sender.send_envelope(b"different"),
        Err(NativeFrameError::Wire(NativeWireError::Invalid))
    );
    client_io.write_budget.store(-1, Ordering::SeqCst);
    sender.send_envelope(&payload).unwrap();
    assert_eq!(receiver.receive_envelope().unwrap(), payload);
    drop(sender);
    assert!(
        receiver.receive_envelope().is_err(),
        "no duplicated complete output"
    );
}

#[test]
fn native_framing_refuses_oversize_before_body_and_accepts_exact() {
    let (server, mut peer, _, _) = peers();
    let mut channel = EnrollmentChannel::new(server);
    // Send only the rejected prefix. A decoder/body read would wait for missing
    // bytes; the actual complete-envelope publication must refuse first.
    peer.write_all(&4097u32.to_be_bytes()).unwrap();
    peer.flush().unwrap();
    assert_eq!(
        channel.receive_envelope(),
        Err(NativeFrameError::Wire(NativeWireError::TooLarge))
    );
    assert_eq!(
        channel.receive_envelope(),
        Err(NativeFrameError::Wire(NativeWireError::TooLarge))
    );
    drop(channel);
    drop(peer);
    let (server, client, _, _) = peers();
    let mut receiver = EnrollmentChannel::new(server);
    let mut sender = EnrollmentChannel::new(client);
    let mut exact = encode_refused().unwrap();
    exact.resize(4096, b' ');
    sender.send_envelope(&exact).unwrap();
    assert_eq!(receiver.receive_envelope().unwrap(), exact);
    assert_eq!(
        sender.send_envelope(&vec![b' '; 4097]),
        Err(NativeFrameError::Wire(NativeWireError::TooLarge))
    );
    let valid = encode_refused().unwrap();
    sender.send_envelope(&valid).unwrap();
    assert_eq!(receiver.receive_envelope().unwrap(), valid);
}
