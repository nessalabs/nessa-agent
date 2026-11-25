//! One selected relay socket from the Join preamble through pinned status.
use super::{ReadStop, StopSocket};
use nessa_auth::adapters::pairing::relay_key;
use nessa_protocol::{
    clock::Clock,
    pairing::socket::{check_deadline, DeadlineStream, NativeDeadline, NativeSocket, WakeEndpoint},
    relay::{read_frame, write_frame, GatewayKey, RelayFrame, RelayRefusal},
};
use std::{
    future::Future,
    io::{Error, ErrorKind, Read, Result, Write},
    net::{SocketAddr, TcpStream},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

/// One caller-owned time budget and stop source, even if the retained
/// operation later opens more than one explicitly selected socket.
pub(crate) struct JoinedEpisode {
    clock: Arc<dyn Clock>,
    stop: Arc<ReadStop>,
    expires_ms: u64,
}

impl JoinedEpisode {
    pub(crate) fn new(clock: Arc<dyn Clock>, stop: Arc<ReadStop>, expires_ms: u64) -> Self {
        Self {
            clock,
            stop,
            expires_ms,
        }
    }

    // This tick polls the one admitted dial future, not repeated connects.
    async fn await_dial<T>(
        &self,
        dial: impl Future<Output = Result<T>>,
        wake: &WakeEndpoint,
    ) -> Result<T> {
        tokio::pin!(dial);
        loop {
            let wait = self.remaining(wake)?;
            tokio::select! {
                result = &mut dial => {
                    self.remaining(wake)?;
                    return result;
                }
                _ = tokio::time::sleep(wait.min(super::STOP_SLICE)) => {}
            }
        }
    }

    fn remaining(&self, wake: &WakeEndpoint) -> Result<Duration> {
        check_stop(&self.stop)?;
        if wake.woken() {
            return Err(ErrorKind::ConnectionAborted.into());
        }
        self.expires_ms
            .checked_sub(self.clock.elapsed_ms())
            .filter(|left| *left > 0)
            .map(Duration::from_millis)
            .ok_or_else(|| ErrorKind::TimedOut.into())
    }
}

// A dial transfers one exact connected socket. Orchestration chooses when and
// how long to poll it, while composition supplies the physical connector.
pub(crate) type JoinedDialFuture = Pin<Box<dyn Future<Output = Result<TcpStream>> + Send>>;
pub(crate) trait JoinedStatusDial: Send + Sync {
    fn dial(&self, address: SocketAddr) -> JoinedDialFuture;
}

pub(crate) struct JoinedStatusRoute {
    address: SocketAddr,
    pin: [u8; 44],
    key: GatewayKey,
    dial: Arc<dyn JoinedStatusDial>,
}

impl JoinedStatusRoute {
    pub(crate) fn new(
        address: SocketAddr,
        pin: [u8; 44],
        dial: Arc<dyn JoinedStatusDial>,
    ) -> Result<Self> {
        if address.port() == 0 {
            return Err(ErrorKind::InvalidInput.into());
        }
        let key = relay_key(&pin).ok_or(ErrorKind::InvalidInput)?;
        Ok(Self {
            address,
            pin,
            key,
            dial,
        })
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    pub(crate) fn pin(&self) -> &[u8; 44] {
        &self.pin
    }

    pub(crate) fn join(
        self,
        episode: &JoinedEpisode,
        wake: Arc<WakeEndpoint>,
        reactor: &tokio::runtime::Handle,
    ) -> Result<(DeadlineStream<JoinedStatusSocket>, Arc<NativeDeadline>)> {
        // Only the already-admitted blocking worker calls this, while the
        // caller runtime is driving status_completed. Do not create a runtime,
        // spawn a second reader, or move a pending dial out of this owner.
        let raw = reactor.block_on(async {
            // Refuse even the port invocation before budget/stop admission.
            episode.remaining(&wake)?;
            episode
                .await_dial(self.dial.dial(self.address), &wake)
                .await
        })?;
        raw.set_nonblocking(false)?;
        let handle = episode.stop.register(&Arc::new(()), &raw)?;
        let socket = JoinedStatusSocket {
            raw,
            stop: episode.stop.clone(),
            handle,
        };
        check_stop(&socket.stop)?;
        socket.raw.set_nodelay(true)?;
        let (mut io, deadline) =
            DeadlineStream::new_absolute(socket, episode.clock.clone(), wake, episode.expires_ms)
                .map_err(deadline_io)?;
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

    #[tokio::test]
    async fn stop_and_registry_follow_the_exact_joined_socket_until_worker_drops_it() {
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
        let route =
            JoinedStatusRoute::new(addr, pin, Arc::new(crate::retained::TokioJoinedDial)).unwrap();
        let reactor = tokio::runtime::Handle::current();
        let worker_stop = stop.clone();
        let (mut joined, _) = tokio::task::spawn_blocking(move || {
            let episode = JoinedEpisode::new(Arc::new(Time), worker_stop, 2500);
            route.join(&episode, WakeEndpoint::new(addr), &reactor)
        })
        .await
        .unwrap()
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
    fn fresh_socket_wake_does_not_extend_the_episode() {
        use std::sync::atomic::{AtomicU64, Ordering};

        struct TestClock(AtomicU64);
        impl Clock for TestClock {
            fn elapsed_ms(&self) -> u64 {
                self.0.load(Ordering::Relaxed)
            }
        }

        let address = SocketAddr::from(([127, 0, 0, 1], 1234));
        let clock = Arc::new(TestClock(AtomicU64::new(500)));
        let stop = ReadStop::new();
        let episode = JoinedEpisode::new(clock.clone(), stop.clone(), 1000);
        assert_eq!(
            episode.remaining(&WakeEndpoint::new(address)).unwrap(),
            Duration::from_millis(500)
        );

        clock.0.store(1000, Ordering::Relaxed);
        let next_wake = WakeEndpoint::new(address);
        assert_eq!(
            episode.remaining(&next_wake).unwrap_err().kind(),
            ErrorKind::TimedOut
        );
        stop.stop();
        assert_eq!(
            episode.remaining(&next_wake).unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
    }

    #[tokio::test]
    async fn expired_or_stopped_attempt_cannot_dial_or_send_relay_preamble() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        for (expires, is_stopped, kind) in [
            (500, false, ErrorKind::TimedOut),
            (1000, true, ErrorKind::ConnectionAborted),
        ] {
            let pin = NativeIdentity::generate(&mut OsEntropy)
                .unwrap()
                .public_spki();
            let route =
                JoinedStatusRoute::new(addr, pin, Arc::new(crate::retained::TokioJoinedDial))
                    .unwrap();
            let stop = ReadStop::new();
            if is_stopped {
                stop.stop();
            }
            let reactor = tokio::runtime::Handle::current();
            let result = tokio::task::spawn_blocking(move || {
                let episode = JoinedEpisode::new(Arc::new(Time), stop, expires);
                route.join(&episode, WakeEndpoint::new(addr), &reactor)
            })
            .await
            .unwrap();
            assert!(matches!(result, Err(err) if err.kind() == kind));
        }
        assert!(matches!(listener.accept(), Err(err) if err.kind() == ErrorKind::WouldBlock));
    }
}
