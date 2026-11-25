//! Private process-fixture adapter for one retained record page.
use super::{
    failure, DeviceEvidence, GatewayConnector, GatewayStream, MonotonicClock, ReadFailure,
    ReadStop, ReaderAccess, RelayRoute, RetainedCache, Scope, StoppableConnector, Stopping,
    HANDSHAKE_MS, PASSIVE_MIN_REQUEST_TIMEOUT_MS,
};
use crate::protected::DeadlineStream;
use crate::read_only_sync::infrastructure::gateway::select_native;
use nessa_auth::{adapters::pairing::NativeTransport, domain::CredentialId};
use nessa_protocol::{
    conversation::read_scope::validate_passive_read_selector,
    pairing::wire::{
        decode_reply, NativePairingReply, NativePairingRequest, RecordCarrierMode,
        RecordCarrierOpen,
    },
};
use nessa_sync::replication::infrastructure::session::{
    CapabilityRecordSession, InterruptibleSessionIo, SessionInterrupt, SessionLimits,
};
use nessa_sync::replication::{
    application::{RecordSource, SourceError},
    domain::{CapabilitySupport, Page, PageRequest},
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

struct RecordIo<'a> {
    transport: NativeTransport<DeadlineStream>,
    interrupt: Arc<dyn SessionInterrupt>,
    cut: Option<&'a Cell<CutState>>,
}
impl Read for RecordIo<'_> {
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
impl Write for RecordIo<'_> {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        self.transport.write(bytes)
    }
    fn flush(&mut self) -> IoResult<()> {
        self.transport.flush()
    }
}
impl InterruptibleSessionIo for RecordIo<'_> {
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
        .map_err(failure)?;
        let reply = selected.receive_envelope().map_err(|_| {
            if ending.stop.is_stopped() {
                ReadFailure::Stopped
            } else {
                ReadFailure::Unreachable
            }
        })?;
        match decode_reply(&reply).map_err(|_| ReadFailure::Protocol)? {
            NativePairingReply::RecordCarrierV1Accepted => {}
            NativePairingReply::Refused => return Err(ReadFailure::Refused),
            _ => return Err(ReadFailure::Protocol),
        }
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
        let session = CapabilityRecordSession::connect(
            io,
            scope.clone(),
            SessionLimits {
                control_bytes: 2048,
                bindings: 1,
            },
            CapabilitySupport::current(),
        )
        .map_err(|_| {
            if ending.stop.is_stopped() {
                ReadFailure::Stopped
            } else {
                ReadFailure::Protocol
            }
        })?;
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
        if result.is_err() && ending.stop.is_stopped() {
            return Err(ReadFailure::Stopped);
        }
        result
    }
}
