//! Private process-fixture adapter for one retained record page.
use super::{
    failure, run_records, CacheError, DeviceEvidence, GatewayConnector, GatewayStream,
    MonotonicClock, ReadFailure, ReadStop, ReaderAccess, RecordCommitIntent, RelayRoute,
    RetainedCache, Scope, StoppableConnector, Stopping, HANDSHAKE_MS,
    PASSIVE_MIN_REQUEST_TIMEOUT_MS,
};
use crate::read_only_sync::infrastructure::gateway::select_native;
use nessa_auth::{adapters::pairing::NativeTransport, domain::CredentialId};
use nessa_protocol::{
    conversation::read_scope::validate_passive_read_selector,
    pairing::{
        wire::{
            decode_reply, NativePairingReply, NativePairingRequest, RecordCarrierMode,
            RecordCarrierOpen,
        },
        NativeFrameError,
    },
    product::generated::{MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES},
};
use nessa_sync::replication::infrastructure::session::{
    CapabilityRecordSession, InterruptibleSessionIo, RecordSessionError, SessionInterrupt,
    SessionLimits,
};
use nessa_sync::replication::{
    application::{Access, RecordSource, ScopeAuthorizer, SourceError},
    domain::{validate_page, CapabilitySupport, Checkpoint, Limits, Page, PageRequest},
};
use std::{
    cell::{Cell, RefCell},
    io::{Error, ErrorKind, Read, Result as IoResult, Write},
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

struct ConnectRecord {
    connector: StoppableConnector,
    interrupt: RefCell<Option<Arc<dyn SessionInterrupt>>>,
}

impl GatewayConnector for ConnectRecord {
    fn connect(&self, address: SocketAddr, timeout: Duration) -> IoResult<Box<dyn GatewayStream>> {
        let socket = self.connector.connect_socket(address, timeout)?;
        self.interrupt.replace(Some(socket.handle.clone()));
        Ok(Box::new(socket))
    }
}

#[derive(Clone, Copy)]
enum CutState {
    Waiting,
    Page(u64),
    Prefix(u64),
    Interrupted(u64),
}

// The one existing driver still owns the request and checkpoint. Arm only for
// its Page, not the selector, Bind, Head, or any prior scope authorization.
struct PageCutSource<'a, S> {
    source: S,
    cut: &'a Cell<CutState>,
}
impl<S: RecordSource> RecordSource for PageCutSource<'_, S> {
    fn head(&mut self, scope: &Scope) -> Result<u64, SourceError> {
        self.source.head(scope)
    }

    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        self.cut.set(CutState::Page(request.after));
        self.source.page(request)
    }
}

struct RecordIo<'a, S: Read + Write> {
    transport: NativeTransport<S>,
    interrupt: Arc<dyn SessionInterrupt>,
    cut: Option<&'a Cell<CutState>>,
}
impl<S: Read + Write> Read for RecordIo<'_, S> {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        if !bytes.is_empty() {
            if let Some(cut) = self.cut {
                match cut.get() {
                    CutState::Page(after) => {
                        // Feed one real decoder byte from the accepted reply.
                        let count = self.transport.read(&mut bytes[..1])?;
                        if count == 1 {
                            cut.set(CutState::Prefix(after));
                        }
                        return Ok(count);
                    }
                    CutState::Prefix(after) => {
                        // The preceding read has returned to the frame owner.
                        // Stop only this socket and do not expose TLS bytes
                        // that it may already hold beyond that first byte.
                        self.interrupt.interrupt()?;
                        cut.set(CutState::Interrupted(after));
                        return Err(Error::from(ErrorKind::ConnectionAborted));
                    }
                    CutState::Interrupted(_) => {
                        return Err(Error::from(ErrorKind::ConnectionAborted))
                    }
                    CutState::Waiting => {}
                }
            }
        }
        self.transport.read(bytes)
    }
}
impl<S: Read + Write> Write for RecordIo<'_, S> {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.transport.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.transport.flush()
    }
}
impl<S: Read + Write> InterruptibleSessionIo for RecordIo<'_, S> {
    fn interrupt_handle(&self) -> Arc<dyn SessionInterrupt> {
        self.interrupt.clone()
    }
}

impl RetainedCache {
    /// Use an independently discovered and authorized exact scope; a fixture
    /// must opt into the gateway carrier. No shipping caller selects it.
    #[doc(hidden)]
    pub fn enrolled_record_page_for_test(
        &mut self,
        access: ReaderAccess<'_>,
        scope: &Scope,
        stop: Arc<ReadStop>,
        budget: Duration,
    ) -> Result<(u64, usize), ReadFailure> {
        self.enrolled_record_page(access, scope, stop, budget, None)
    }

    /// Interrupt a real Page reply after one byte entered the frame decoder.
    /// The receipt is the driver's requested offset, only if interrupt returned
    /// success; it is not evidence about what the remote side sent or retained.
    #[doc(hidden)]
    pub fn interrupt_enrolled_record_page_for_test(
        &mut self,
        access: ReaderAccess<'_>,
        scope: &Scope,
        stop: Arc<ReadStop>,
        budget: Duration,
    ) -> (Option<u64>, Result<(u64, usize), ReadFailure>) {
        let cut = Cell::new(CutState::Waiting);
        let result = self.enrolled_record_page(access, scope, stop, budget, Some(&cut));
        let after = match cut.get() {
            CutState::Interrupted(after) => Some(after),
            _ => None,
        };
        (after, result)
    }

    fn enrolled_record_page(
        &mut self,
        access: ReaderAccess<'_>,
        scope: &Scope,
        stop: Arc<ReadStop>,
        budget: Duration,
        cut: Option<&Cell<CutState>>,
    ) -> Result<(u64, usize), ReadFailure> {
        validate_passive_read_selector(access.receiver, access.access_epoch, scope)
            .map_err(|_| ReadFailure::Refused)?;
        let credential =
            CredentialId::new(access.credential.to_owned()).map_err(|_| ReadFailure::Protocol)?;
        let clock: Arc<dyn MonotonicClock> = self.clock.clone();
        let deadline = clock
            .elapsed_ms()
            .saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX));
        let ending = Arc::new(Stopping::new(stop, clock.clone(), deadline));
        let relay = access
            .relay
            .map(|address| {
                Ok(RelayRoute {
                    address: address.clone(),
                    key: super::relay_key(&access.pin).ok_or(ReadFailure::Protocol)?,
                })
            })
            .transpose()?;
        let connector = ConnectRecord {
            connector: StoppableConnector {
                ending: ending.clone(),
                relay,
                offline: Arc::new(AtomicBool::new(false)),
            },
            interrupt: RefCell::new(None),
        };
        if ending.stop.is_stopped() {
            return Err(ReadFailure::Stopped);
        }
        let mut selected = select_native(
            access.address,
            &DeviceEvidence {
                identity: access.identity,
                pin: access.pin,
                credential: access.credential,
            },
            &connector,
            clock.clone(),
            ending.clone(),
            HANDSHAKE_MS,
            &NativePairingRequest::OpenRecordCarrierV1(RecordCarrierOpen {
                credential,
                access_epoch: access.access_epoch,
                scope: scope.clone(),
                mode: RecordCarrierMode::Page,
            }),
        )
        .map_err(|error| match failure(error) {
            // After the explicit preflight, the stock opener cannot tell
            // whether cancellation was a local stop or a budget cut.
            ReadFailure::Stopped => ReadFailure::Unreachable,
            other => other,
        })?;
        accept_selection(selected.receive_envelope())?;
        let mut transport = selected.into_transport();
        let stream = transport.stream_mut();
        stream.begin_operation(
            clock
                .elapsed_ms()
                .saturating_add(PASSIVE_MIN_REQUEST_TIMEOUT_MS),
        );
        let io = RecordIo {
            transport,
            interrupt: connector
                .interrupt
                .into_inner()
                .ok_or(ReadFailure::Protocol)?,
            cut,
        };
        self.page_on_selected_carrier(scope, io)
    }

    fn page_on_selected_carrier<S: Read + Write>(
        &mut self,
        scope: &Scope,
        io: RecordIo<'_, S>,
    ) -> Result<(u64, usize), ReadFailure> {
        let cut = io.cut;
        let session = bind_fixture(io, scope)?;
        let (mut authorizer, mut source) = session.parts();
        let result = match cut {
            Some(cut) => self.one_record_page_for_test(
                scope,
                &mut authorizer,
                &mut PageCutSource { source, cut },
            ),
            None => self.one_record_page_for_test(scope, &mut authorizer, &mut source),
        };
        // The driver is the sole cache owner. A teardown error must not
        // change the meaning of a previously successful durable commit.
        let _ = session.close();
        result
    }

    pub(super) fn retain_selected_page_for_test<S: Read + Write, P>(
        &mut self,
        transport: NativeTransport<S>,
        interrupt: Arc<dyn SessionInterrupt>,
        scope: &Scope,
        claim: impl FnOnce(RecordCommitIntent<'_>) -> Result<P, ReadFailure>,
    ) -> Result<(u64, usize), ReadFailure> {
        let session = bind_fixture(
            RecordIo {
                transport,
                interrupt,
                cut: None,
            },
            scope,
        )?;
        let (mut authorizer, mut source) = session.parts();
        let policy = self.policy.suffix_page();
        let limits = Limits::new(1, policy.max_payload_bytes(), policy.max_record_bytes())
            .map_err(|_| ReadFailure::Protocol)?;
        let refusal = Cell::new(None);
        let run = {
            let mut records = self.cache.records_with_admission(|intent| {
                claim(intent).map_err(|reason| {
                    refusal.set(Some(reason));
                    CacheError::Unavailable
                })
            });
            run_records(
                scope,
                &mut authorizer,
                &mut source,
                &mut records,
                limits,
                1,
                self.wall.as_ref(),
            )
        };
        // An already admitted durable outcome outlives the transport.
        let _ = session.close();
        match refusal.get() {
            Some(reason) => Err(reason),
            None => self.record_fixture_result(run),
        }
    }
}

fn bind_fixture<'a, S: Read + Write>(
    io: RecordIo<'a, S>,
    scope: &Scope,
) -> Result<CapabilityRecordSession<RecordIo<'a, S>>, ReadFailure> {
    CapabilityRecordSession::connect(
        io,
        scope.clone(),
        SessionLimits {
            control_bytes: 2048,
            bindings: 1,
        },
        CapabilitySupport::current(),
    )
    .map_err(bind_failure)
}

/// Read one verified Page on the accepted fixture transport. No store is
/// supplied, so validation cannot publish the returned copy of the records.
pub(super) fn read_selected_page_for_test<S: Read + Write>(
    transport: NativeTransport<S>,
    interrupt: Arc<dyn SessionInterrupt>,
    scope: &Scope,
    fence: impl Fn() -> Result<(), ReadFailure>,
) -> Result<Page, ReadFailure> {
    let io = RecordIo {
        transport,
        interrupt,
        cut: None,
    };
    let session = bind_fixture(io, scope)?;
    let result = (|| {
        fence()?;
        let (mut authorizer, mut source) = session.parts();
        if !matches!(authorizer.authorize(scope), Access::Allowed(actual) if actual == *scope) {
            return Err(ReadFailure::Refused);
        }
        let map_source = |err| bind_failure(RecordSessionError::Source(err));
        let target = source.head(scope).map_err(map_source)?;
        fence()?;
        let limits = Limits::new(
            1,
            MAX_RECORD_PAGE_PAYLOAD_BYTES,
            MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        )
        .map_err(|_| ReadFailure::Protocol)?;
        let request = PageRequest {
            scope: scope.clone(),
            after: 0,
            target,
            max_records: limits.max_records(),
            max_payload_bytes: limits.max_payload_bytes(),
            max_record_bytes: limits.max_record_bytes(),
        };
        let page = source.page(&request).map_err(map_source)?;
        fence()?;
        validate_page(
            &Checkpoint::new(scope.clone(), 0),
            &request,
            page.clone(),
            limits,
        )
        .map_err(|_| ReadFailure::Protocol)?;
        Ok(page)
    })();
    let closed = session.close();
    let page = result?;
    closed.map_err(bind_failure)?;
    fence()?;
    Ok(page)
}

pub(super) fn accept_selection(
    frame: Result<Vec<u8>, NativeFrameError>,
) -> Result<(), ReadFailure> {
    let reply = frame.map_err(|error| match error {
        NativeFrameError::Wire(_) => ReadFailure::Protocol,
        NativeFrameError::Io(_) => ReadFailure::Unreachable,
    })?;
    match decode_reply(&reply).map_err(|_| ReadFailure::Protocol)? {
        NativePairingReply::RecordCarrierV1Accepted => Ok(()),
        NativePairingReply::Refused => Err(ReadFailure::Refused),
        _ => Err(ReadFailure::Protocol),
    }
}

fn bind_failure(error: RecordSessionError) -> ReadFailure {
    match error {
        RecordSessionError::Source(SourceError::Denied) => ReadFailure::Refused,
        RecordSessionError::Source(SourceError::IdentityChanged | SourceError::Pruned) => {
            ReadFailure::ResetRequired
        }
        RecordSessionError::Source(SourceError::Unavailable)
        | RecordSessionError::Closed
        | RecordSessionError::Io(_) => ReadFailure::Unreachable,
        _ => ReadFailure::Protocol,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_protocol::pairing::wire::{encode_record_carrier_accepted, encode_refused};

    #[test]
    fn only_an_explicit_selector_acceptance_admits_bind() {
        assert_eq!(
            accept_selection(Ok(encode_record_carrier_accepted().unwrap())),
            Ok(())
        );
        assert_eq!(
            accept_selection(Ok(encode_refused().unwrap())),
            Err(ReadFailure::Refused)
        );
        assert_eq!(accept_selection(Ok(Vec::new())), Err(ReadFailure::Protocol));
        for kind in [
            ErrorKind::UnexpectedEof,
            ErrorKind::ConnectionReset,
            ErrorKind::TimedOut,
        ] {
            assert_eq!(
                accept_selection(Err(NativeFrameError::Io(kind))),
                Err(ReadFailure::Unreachable)
            );
        }
    }

    #[test]
    fn bind_preserves_authoritative_status_but_never_guesses_a_stop() {
        for (status, expected) in [
            (SourceError::Denied, ReadFailure::Refused),
            (SourceError::IdentityChanged, ReadFailure::ResetRequired),
            (SourceError::Pruned, ReadFailure::ResetRequired),
            (SourceError::Unavailable, ReadFailure::Unreachable),
        ] {
            assert_eq!(bind_failure(RecordSessionError::Source(status)), expected);
        }
        assert_eq!(
            bind_failure(RecordSessionError::Closed),
            ReadFailure::Unreachable
        );
        for kind in [
            ErrorKind::UnexpectedEof,
            ErrorKind::ConnectionReset,
            ErrorKind::TimedOut,
        ] {
            assert_eq!(
                bind_failure(RecordSessionError::Io(Error::from(kind))),
                ReadFailure::Unreachable
            );
        }
    }
}
