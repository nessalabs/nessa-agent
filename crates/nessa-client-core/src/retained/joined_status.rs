//! One selected relay socket from the Join preamble through pinned status.
use super::{ReadFailure, ReadStop, RecordCommitIntent, RetainedCache, StopSocket};
use crate::pairing::NativeClientError;
use nessa_auth::{
    adapters::pairing::{relay_key, GatewayTrust, NativeTransport},
    domain::CredentialId,
};
use nessa_protocol::{
    clock::Clock,
    conversation::read_scope::validate_passive_read_selector,
    pairing::{
        socket::{check_deadline, DeadlineStream, NativeDeadline, NativeSocket, WakeEndpoint},
        wire::{
            encode_request, NativePairingRequest, NativePairingStatus, RecordCarrierMode,
            RecordCarrierOpen,
        },
        EnrollmentChannel, NativeFrameError,
    },
    relay::{read_frame, write_frame, GatewayKey, RelayFrame, RelayRefusal},
};
use nessa_sync::replication::{
    domain::Page,
    infrastructure::session::SessionInterrupt,
};
use std::{
    future::Future,
    io::{Error, ErrorKind, Read, Result, Write},
    net::{SocketAddr, TcpStream},
    pin::Pin,
    sync::{Arc, MutexGuard},
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

    fn claim_publication(&self, wake: &WakeEndpoint) -> Result<MutexGuard<'_, ()>, ReadFailure> {
        let permit = self.stop.publication_lock();
        if self.stop.is_stopped() {
            return Err(ReadFailure::Stopped);
        }
        self.remaining(wake).map_err(|_| ReadFailure::Unreachable)?;
        Ok(permit)
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

// The stream remains the sole registration owner. Consuming this result
// hands the next protocol the original stream, never a rewrapped raw socket.
pub(crate) type JoinedStream = (
    DeadlineStream<JoinedStatusSocket>,
    Arc<NativeDeadline>,
    Arc<dyn SessionInterrupt>,
);

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
    ) -> Result<JoinedStream> {
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
        let interrupt = handle.clone();
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
                Ok((io, deadline, interrupt))
            }
            other => Err(RelayRefusal::of(other)
                .map_or_else(|| Error::from(ErrorKind::InvalidData), Error::from)),
        }
    }
}

/// Test-support inputs for one physical pinned Status episode.
pub(crate) struct JoinedStatusFixture {
    pub(crate) route: JoinedStatusRoute,
    pub(crate) stop: Arc<ReadStop>,
    pub(crate) expires_ms: u64,
    pub(crate) continuation: Option<Arc<WakeEndpoint>>,
}

/// The exact selected route and scopes carry the existing episode forward.
pub(crate) struct JoinedRecordFixture<'a> {
    pub(crate) trusted_scope: &'a super::Scope,
    pub(crate) requested_scope: &'a super::Scope,
    pub(crate) route: JoinedStatusRoute,
    pub(crate) episode: JoinedEpisode,
    pub(crate) wake: Arc<WakeEndpoint>,
}

pub(crate) struct JoinedRecordSelection {
    joined: JoinedStream,
    reader: super::RetainedDeviceReader,
    address: SocketAddr,
    scope: super::Scope,
    episode: JoinedEpisode,
    wake: Arc<WakeEndpoint>,
}

// Synthetic composition only until the retained cache-owning entry point
// can consume this proof. Successful validation does not itself grant access.
pub(crate) fn join_after_active_for_test(
    status: std::result::Result<NativePairingStatus, NativeClientError>,
    state: &dyn nessa_auth::application::pairing::ClientPendingStore,
    fixture: JoinedRecordFixture<'_>,
    reactor: &tokio::runtime::Handle,
) -> std::result::Result<JoinedRecordSelection, NativeClientError> {
    let JoinedRecordFixture {
        trusted_scope,
        requested_scope,
        route,
        episode,
        wake,
    } = fixture;
    let status = status?;
    let reader = super::RetainedDeviceReader::from_current(&status, state)?
        .ok_or(NativeClientError::Refused)?;
    let address = route.address();
    let access = reader.access(address, "joined-record-fixture");
    if &access.pin != route.pin() || trusted_scope != requested_scope {
        return Err(NativeClientError::Refused);
    }
    validate_passive_read_selector(access.receiver, access.access_epoch, trusted_scope)
        .map_err(|_| NativeClientError::Refused)?;
    let joined = route
        .join(&episode, wake.clone(), reactor)
        .map_err(|error| NativeClientError::Io(error.kind()))?;
    Ok(JoinedRecordSelection {
        joined,
        reader,
        address,
        scope: trusted_scope.clone(),
        episode,
        wake,
    })
}

// Keep the selector-only unit matrix on the same framing and fence.
#[cfg(test)]
pub(crate) fn select_joined_record_for_test(
    selection: JoinedRecordSelection,
    after_accepted: impl FnOnce(),
) -> std::result::Result<(), ReadFailure> {
    with_selected_joined_record(selection, after_accepted, |_, _, _, _, _| Ok(()))
}

pub(super) fn read_joined_record_page_for_test(
    selection: JoinedRecordSelection,
) -> std::result::Result<Page, ReadFailure> {
    with_selected_joined_record(
        selection,
        || {},
        |transport, interrupt, scope, episode, wake| {
            super::carrier::read_selected_page_for_test(transport, interrupt, scope, || {
                episode.remaining(wake).map(|_| ()).map_err(|_| ReadFailure::Unreachable)
            })
        },
    )
}

pub(super) fn retain_joined_record_page_for_test(
    selection: JoinedRecordSelection,
    open_cache: impl FnOnce() -> std::result::Result<RetainedCache, ReadFailure>,
    before_commit: impl FnOnce(RecordCommitIntent<'_>),
) -> std::result::Result<(u64, usize), ReadFailure> {
    with_selected_joined_record(
        selection,
        || {},
        |transport, interrupt, scope, episode, wake| {
            let mut cache = open_cache()?;
            cache.retain_selected_page_for_test(transport, interrupt, scope, |intent| {
                before_commit(intent);
                episode.claim_publication(wake)
            })
        },
    )
}

fn with_selected_joined_record<T>(
    selection: JoinedRecordSelection,
    after_accepted: impl FnOnce(),
    use_transport: impl FnOnce(
        NativeTransport<DeadlineStream<JoinedStatusSocket>>,
        Arc<dyn SessionInterrupt>,
        &super::Scope,
        &JoinedEpisode,
        &WakeEndpoint,
    ) -> std::result::Result<T, ReadFailure>,
) -> std::result::Result<T, ReadFailure> {
    let JoinedRecordSelection {
        joined,
        reader,
        address,
        scope,
        episode,
        wake,
    } = selection;
    let (stream, _deadline, interrupt) = joined;
    let access = reader.access(address, "joined-record-fixture");
    let transport =
        NativeTransport::connect(stream, access.identity, GatewayTrust::Pinned(access.pin))
            .map_err(|_| ReadFailure::Unreachable)?;
    let mut channel = EnrollmentChannel::new(transport);
    let request = NativePairingRequest::OpenRecordCarrierV1(RecordCarrierOpen {
        credential: CredentialId::new(access.credential.to_owned())
            .map_err(|_| ReadFailure::Protocol)?,
        access_epoch: access.access_epoch,
        scope: scope.clone(),
        mode: RecordCarrierMode::Page,
    });
    let envelope = encode_request(&request).map_err(|_| ReadFailure::Protocol)?;
    channel
        .send_envelope(&envelope)
        .map_err(|error| match error {
            NativeFrameError::Wire(_) => ReadFailure::Protocol,
            NativeFrameError::Io(_) => ReadFailure::Unreachable,
        })?;
    super::carrier::accept_selection(channel.receive_envelope())?;
    after_accepted();
    // TLS may have buffered the plaintext before stop/wake/expiry changed.
    // Recheck the same episode and exact wake, not just the socket deadline.
    episode.remaining(&wake).map_err(|_| ReadFailure::Unreachable)?;
    use_transport(channel.into_transport(), interrupt, &scope, &episode, &wake)
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
        let (mut joined, _, interrupt) = tokio::task::spawn_blocking(move || {
            let episode = JoinedEpisode::new(Arc::new(Time), worker_stop, 2500);
            route.join(&episode, WakeEndpoint::new(addr), &reactor)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(stop.lock().len(), 1);
        interrupt.interrupt().unwrap();
        assert!(
            !stop.is_stopped(),
            "socket interruption must not stop the episode"
        );
        assert_eq!(stop.lock().len(), 1);
        stop.stop();
        assert_eq!(
            joined.write(b"first TLS byte").unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
        drop(joined);
        assert!(stop.lock().is_empty(), "dropping the stream unregisters it");
        drop(interrupt);
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
