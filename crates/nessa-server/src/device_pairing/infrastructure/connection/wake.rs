//! Shutdown wake-ups for sockets whose worker may be blocked in a read.
//!
//! Registering an endpoint grants no admission: the caller must already hold a
//! connection permit. A wake outcome records the `shutdown(2)` result only; it
//! does not say the worker has finished. Drain is reported separately by each
//! owner's `shutdown`.
use std::{
    io::{ErrorKind, Result as IoResult},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{Arc, Mutex, PoisonError, Weak},
};

/// Bound shared by native connection admission and its wake report.
pub(in crate::device_pairing::infrastructure) const NATIVE_CONNECTION_CAPACITY: usize = 8;

/// Why a blocked socket was woken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeWakeCause {
    /// The owner stopped admitting connections and is waiting for workers to drain.
    AdmissionRetirement,
}

/// `shutdown(2)` result for one admitted socket; not evidence that its worker ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeWakeOutcome {
    target: SocketAddr,
    cause: NativeWakeCause,
    result: Result<(), ErrorKind>,
}
impl NativeWakeOutcome {
    /// Peer address of the woken socket.
    pub fn target(&self) -> SocketAddr {
        self.target
    }
    /// Lifecycle cause, kept separately from the syscall result.
    pub fn cause(&self) -> NativeWakeCause {
        self.cause
    }
    /// The syscall result for this socket.
    pub fn result(&self) -> Result<(), ErrorKind> {
        self.result
    }
}

/// The one wake sweep an owner performs when it stops admitting connections.
/// `native_shutdown_keeps_original_physical_capacity` checks that a repeated
/// shutdown returns the same report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeWakeReport {
    entries: [Option<NativeWakeOutcome>; NATIVE_CONNECTION_CAPACITY],
}
impl NativeWakeReport {
    /// One outcome per socket that was still open at the sweep, successes included.
    pub fn iter(&self) -> impl Iterator<Item = &NativeWakeOutcome> {
        self.entries.iter().flatten()
    }
}

struct EndpointState {
    socket: Option<TcpStream>,
    wake: Option<NativeWakeOutcome>,
}
pub(in crate::device_pairing::infrastructure) struct WakeEndpoint {
    target: SocketAddr,
    state: Mutex<EndpointState>,
}
impl WakeEndpoint {
    pub(in crate::device_pairing::infrastructure) fn new(socket: TcpStream) -> IoResult<Arc<Self>> {
        let target = socket.peer_addr()?;
        Ok(Arc::new(Self {
            target,
            state: Mutex::new(EndpointState {
                socket: Some(socket),
                wake: None,
            }),
        }))
    }
    /// The worker has stopped using the socket. Closing our duplicate here keeps a
    /// later sweep from calling shutdown on a descriptor number the OS has reused.
    /// It does not release the connection permit.
    pub(in crate::device_pairing::infrastructure) fn physically_returned(&self) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .socket
            .take();
    }
    fn wake(&self) -> Option<NativeWakeOutcome> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.wake.is_none() {
            if let Some(socket) = &state.socket {
                let result = socket
                    .shutdown(Shutdown::Both)
                    .map_err(|error| error.kind());
                state.wake = Some(NativeWakeOutcome {
                    target: self.target,
                    cause: NativeWakeCause::AdmissionRetirement,
                    result,
                });
            }
        }
        state.wake
    }
}
pub(in crate::device_pairing::infrastructure) struct WakeEndpoints {
    active: Vec<Weak<WakeEndpoint>>,
    report: Option<NativeWakeReport>,
}
impl WakeEndpoints {
    pub(in crate::device_pairing::infrastructure) fn new() -> Self {
        Self {
            active: Vec::with_capacity(NATIVE_CONNECTION_CAPACITY),
            report: None,
        }
    }
    pub(in crate::device_pairing::infrastructure) fn register(
        &mut self,
        endpoint: &Arc<WakeEndpoint>,
    ) {
        self.active.retain(|endpoint| endpoint.strong_count() != 0);
        assert!(
            self.active.len() < NATIVE_CONNECTION_CAPACITY,
            "registration requires a held connection permit"
        );
        self.active.push(Arc::downgrade(endpoint));
    }
    pub(in crate::device_pairing::infrastructure) fn report(&self) -> Option<NativeWakeReport> {
        self.report
    }
    /// Callers close their semaphore while holding the lock that guards this
    /// value, and admit only while holding it, so no socket is registered after
    /// the sweep. A second call returns the first report without another syscall.
    pub(in crate::device_pairing::infrastructure) fn close(&mut self) -> NativeWakeReport {
        *self.report.get_or_insert_with(|| {
            let mut report = NativeWakeReport {
                entries: [None; NATIVE_CONNECTION_CAPACITY],
            };
            for (index, endpoint) in self.active.iter().filter_map(Weak::upgrade).enumerate() {
                report.entries[index] = endpoint.wake();
            }
            report
        })
    }
}
