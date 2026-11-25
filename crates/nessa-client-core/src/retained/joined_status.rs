//! One selected relay socket from the Join preamble through pinned status.
use super::{ReadStop, StopSocket};
use nessa_auth::adapters::pairing::relay_key;
use nessa_protocol::{
    clock::Clock,
    pairing::socket::{check_deadline, DeadlineStream, NativeDeadline, NativeSocket, WakeEndpoint},
    relay::{read_frame, write_frame, GatewayKey, RelayFrame, RelayRefusal},
};
use std::{
    io::{Error, ErrorKind, Read, Result, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

// A single numeric blocking connect cannot be interrupted portably before
// the OS yields a socket. Cap that one physical dial; never emulate stop
// polling by opening repeated sockets. Full stop checks start after connect.
const MAX_BLOCKING_DIAL: Duration = Duration::from_millis(250);

fn dial_budget(now: u64, expires_ms: u64) -> Result<Duration> {
    expires_ms
        .checked_sub(now)
        .filter(|left| *left > 0)
        .map(Duration::from_millis)
        .map(|remaining| remaining.min(MAX_BLOCKING_DIAL))
        .ok_or_else(|| ErrorKind::TimedOut.into())
}

pub(crate) struct JoinedStatusRoute {
    address: SocketAddr,
    pin: [u8; 44],
    key: GatewayKey,
}

impl JoinedStatusRoute {
    pub(crate) fn new(address: SocketAddr, pin: [u8; 44]) -> Result<Self> {
        if address.port() == 0 {
            return Err(ErrorKind::InvalidInput.into());
        }
        let key = relay_key(&pin).ok_or(ErrorKind::InvalidInput)?;
        Ok(Self { address, pin, key })
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn pin(&self) -> &[u8; 44] {
        &self.pin
    }

    pub(crate) fn join(
        self,
        clock: Arc<dyn Clock>,
        stop: Arc<ReadStop>,
        wake: Arc<WakeEndpoint>,
        expires_ms: u64,
    ) -> Result<(DeadlineStream<JoinedStatusSocket>, Arc<NativeDeadline>)> {
        check_stop(&stop)?;
        if wake.woken() {
            return Err(ErrorKind::ConnectionAborted.into());
        }
        let raw = TcpStream::connect_timeout(
            &self.address,
            dial_budget(clock.elapsed_ms(), expires_ms)?,
        )?;
        let handle = stop.register(&Arc::new(()), &raw)?;
        let socket = JoinedStatusSocket { raw, stop, handle };
        check_stop(&socket.stop)?;
        socket.raw.set_nodelay(true)?;
        let (mut io, deadline) =
            DeadlineStream::new_absolute(socket, clock, wake, expires_ms).map_err(deadline_io)?;
        write_frame(&mut io, &RelayFrame::Connect { key: self.key })?;
        match read_frame(&mut io)? {
            RelayFrame::Joined => {
                check_deadline(&deadline).map_err(deadline_io)?;
                Ok((io, deadline))
            }
            other => Err(RelayRefusal::of(other)
                .map_or_else(|| Error::from(ErrorKind::InvalidData), Error::from)),
        }
    }
}

// The enrollment worker owns this exact socket and registration from Connect
// through pinned status. A dropped waiter doesn't release its physical ticket.
pub(crate) struct JoinedStatusSocket {
    raw: TcpStream,
    stop: Arc<ReadStop>,
    handle: Arc<StopSocket>,
}
impl Drop for JoinedStatusSocket {
    fn drop(&mut self) {
        self.stop.forget(&self.handle);
    }
}
fn check_stop(stop: &ReadStop) -> Result<()> {
    if stop.is_stopped() {
        Err(ErrorKind::ConnectionAborted.into())
    } else {
        Ok(())
    }
}
impl Read for JoinedStatusSocket {
    fn read(&mut self, data: &mut [u8]) -> Result<usize> {
        check_stop(&self.stop)?;
        let result = self.raw.read(data);
        check_stop(&self.stop)?;
        result
    }
}
impl Write for JoinedStatusSocket {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        check_stop(&self.stop)?;
        self.raw.write(data)
    }
    fn flush(&mut self) -> Result<()> {
        check_stop(&self.stop)?;
        self.raw.flush()
    }
}
impl NativeSocket for JoinedStatusSocket {
    fn set_nonblocking(&self, enabled: bool) -> Result<()> {
        check_stop(&self.stop)?;
        let result = self.raw.set_nonblocking(enabled);
        check_stop(&self.stop)?;
        result
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        check_stop(&self.stop)?;
        let result = self.raw.set_read_timeout(timeout);
        check_stop(&self.stop)?;
        result
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<()> {
        check_stop(&self.stop)?;
        let result = self.raw.set_write_timeout(timeout);
        check_stop(&self.stop)?;
        result
    }
}

fn deadline_io(error: nessa_protocol::pairing::socket::DeadlineError) -> Error {
    use nessa_protocol::pairing::socket::DeadlineError;
    match error {
        DeadlineError::Io(kind) => Error::from(kind),
        DeadlineError::Phase => Error::from(ErrorKind::InvalidData),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_auth::adapters::pairing::{NativeIdentity, OsEntropy};
    use std::{net::TcpListener, thread};

    struct Time;
    impl Clock for Time {
        fn elapsed_ms(&self) -> u64 {
            500
        }
    }

    #[test]
    fn stop_and_registry_follow_the_exact_joined_socket_until_worker_drops_it() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let pin = NativeIdentity::generate(&mut OsEntropy)
            .unwrap()
            .public_spki();
        let key = relay_key(&pin).unwrap();
        let peer = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            assert!(
                matches!(read_frame(&mut socket), Ok(RelayFrame::Connect {key: observed}) if observed==key)
            );
            write_frame(&mut socket, &RelayFrame::Joined).unwrap();
            let mut byte = [0];
            assert!(!matches!(socket.read(&mut byte), Ok(1)));
        });
        let stop = ReadStop::new();
        let route = JoinedStatusRoute::new(addr, pin).unwrap();
        let (mut joined, _) = route
            .join(Arc::new(Time), stop.clone(), WakeEndpoint::new(addr), 2500)
            .unwrap();
        assert_eq!(stop.lock().len(), 1);
        stop.stop();
        assert_eq!(
            joined.write(b"first TLS byte").unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
        drop(joined);
        assert!(stop.lock().is_empty());
        peer.join().unwrap();
    }

    #[test]
    fn numeric_blocking_dial_never_owns_the_entire_long_status_budget() {
        assert_eq!(dial_budget(500, 10_000).unwrap(), MAX_BLOCKING_DIAL);
        assert_eq!(dial_budget(500, 510).unwrap(), Duration::from_millis(10));
        assert_eq!(
            dial_budget(500, 500).unwrap_err().kind(),
            ErrorKind::TimedOut
        );
    }

    #[test]
    fn expired_attempt_cannot_dial_relay() {
        let pin = NativeIdentity::generate(&mut OsEntropy)
            .unwrap()
            .public_spki();
        let addr = "127.0.0.1:9".parse().unwrap();
        let route = JoinedStatusRoute::new(addr, pin).unwrap();
        let result = route.join(
            Arc::new(Time),
            ReadStop::new(),
            WakeEndpoint::new(addr),
            500,
        );
        assert!(matches!(result,Err(err) if err.kind()==ErrorKind::TimedOut));
    }
}
