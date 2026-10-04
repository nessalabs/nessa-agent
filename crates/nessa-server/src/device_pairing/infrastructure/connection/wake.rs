//! Shutdown wake-ups for workers that may be blocked on their socket.
//!
//! A wake sets a per-socket stop flag. It makes no socket call from another
//! thread: on Windows, `shutdown` from another thread does not interrupt a
//! blocking receive, so the same flag is the mechanism on every OS. The
//! worker's `DeadlineStream` caps each read wait at [`WAKE_TICK`] and checks the
//! flag between waits, so a woken worker's read fails within one tick. A send
//! in progress is not interrupted: it ends at its phase deadline.
//!
//! Registering an endpoint grants no admission: the caller must already hold a
//! connection permit. A wake outcome does not say the worker has finished;
//! drain is reported separately by each owner's `shutdown`.
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, PoisonError, Weak},
    task::Waker,
    time::Duration,
};

/// Bound shared by native connection admission and its wake report.
pub(in crate::device_pairing::infrastructure) const NATIVE_CONNECTION_CAPACITY: usize = 8;
/// Bound on protected product sessions, a pool of their own so sessions never
/// take the permits enrollment and pinned status need (design row PR10).
/// Equal to the connection bound, which also sizes the wake report.
pub(in crate::device_pairing::infrastructure) const PRODUCT_SESSION_CAPACITY: usize =
    NATIVE_CONNECTION_CAPACITY;
/// Longest a blocked socket wait lasts before it checks for a wake again.
pub(in crate::device_pairing::infrastructure) const WAKE_TICK: Duration =
    Duration::from_millis(100);

/// Why a blocked socket was woken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeWakeCause {
    /// The owner stopped admitting connections and is waiting for workers to drain.
    AdmissionRetirement,
}

/// One connection whose permit was still held when its owner woke it. Not
/// evidence that its worker has finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeWakeOutcome {
    target: SocketAddr,
    cause: NativeWakeCause,
}
impl NativeWakeOutcome {
    /// Peer address of the woken socket.
    pub fn target(&self) -> SocketAddr {
        self.target
    }
    /// Lifecycle cause of the wake.
    pub fn cause(&self) -> NativeWakeCause {
        self.cause
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
    /// One outcome per connection whose permit was still held at the sweep.
    /// An endpoint lives exactly as long as its connection permit.
    pub fn iter(&self) -> impl Iterator<Item = &NativeWakeOutcome> {
        self.entries.iter().flatten()
    }
}

struct EndpointState {
    wake: Option<NativeWakeOutcome>,
    /// The async task reading this connection, woken with the endpoint, so
    /// an idle protected session sees a shutdown at once.
    reader: Option<Waker>,
}
pub(in crate::device_pairing::infrastructure) struct WakeEndpoint {
    target: SocketAddr,
    state: Mutex<EndpointState>,
}
impl WakeEndpoint {
    /// `target` is the peer address of the connection's socket.
    pub(in crate::device_pairing::infrastructure) fn new(target: SocketAddr) -> Arc<Self> {
        Arc::new(Self {
            target,
            state: Mutex::new(EndpointState {
                wake: None,
                reader: None,
            }),
        })
    }
    /// Whether the owner has woken this socket; its IO must stop.
    pub(in crate::device_pairing::infrastructure) fn woken(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .wake
            .is_some()
    }
    /// Have `reader` woken when this endpoint is woken; `true` if it already
    /// was. Registering and checking happen under one lock, so a wake cannot
    /// fall between them.
    pub(in crate::device_pairing::infrastructure) fn wake_reader(&self, reader: &Waker) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.wake.is_some() {
            return true;
        }
        if !state
            .reader
            .as_ref()
            .is_some_and(|known| known.will_wake(reader))
        {
            state.reader = Some(reader.clone());
        }
        false
    }
    fn wake(&self) -> Option<NativeWakeOutcome> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.wake.is_none() {
            state.wake = Some(NativeWakeOutcome {
                target: self.target,
                cause: NativeWakeCause::AdmissionRetirement,
            });
        }
        let outcome = state.wake;
        let reader = state.reader.take();
        drop(state);
        if let Some(reader) = reader {
            reader.wake();
        }
        outcome
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
    /// Stop sweeping `endpoint`: its connection moved to another owner.
    pub(in crate::device_pairing::infrastructure) fn remove(
        &mut self,
        endpoint: &Arc<WakeEndpoint>,
    ) {
        self.active
            .retain(|known| !std::ptr::eq(known.as_ptr(), Arc::as_ptr(endpoint)));
    }
    pub(in crate::device_pairing::infrastructure) fn report(&self) -> Option<NativeWakeReport> {
        self.report
    }
    /// Callers close their semaphore while holding the lock that guards this
    /// value, and admit only while holding it, so no socket is registered after
    /// the sweep. A second call returns the first report.
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
