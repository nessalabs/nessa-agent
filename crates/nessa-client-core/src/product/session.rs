//! Original tickets and cancellation-independent physical ownership.
use super::ports::{ProductConnector, ProductInterrupt, ProductWake};
use super::worker::{self, Open};
use crate::read_only_sync::application::GatewayError;
use nessa_auth::adapters::pairing::NativeIdentity;
use nessa_protocol::clock::Clock;
use nessa_protocol::pairing::{MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES};
use nessa_protocol::product::generated::ProductSessionReady;
use nessa_protocol::product::passive_read::{encode_request, ReadEncodeError};
use nessa_protocol::product_contract::generated::SessionCloseReason;
use nessa_protocol::product_contract::{connection_outcome, ConnectionOutcome};
use nessa_protocol::protocol::{unique_value, EventFrame, RequestFrame, ResponseFrame};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::ops::Deref;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Native transport refusal, distinct from a factual gateway response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductFailure {
    /// Operational limits must be positive and their arithmetic representable.
    InvalidLimits,
    /// Malformed outer request or public handshake metadata.
    InvalidRequest,
    /// The actual Ready did not advertise this method.
    UnadvertisedMethod,
    /// Native admission or event capacity exhausted.
    Capacity,
    /// The directional published request bound was exceeded.
    RequestTooLarge,
    /// The directional published response bound was exceeded.
    ResponseTooLarge,
    /// Absolute handshake, request or partial-frame deadline elapsed.
    TimedOut,
    /// The physical owner was explicitly stopped.
    Cancelled,
    /// Physical socket failure.
    Transport,
    /// Gateway closed with its original typed reason, when available.
    Closed(Option<SessionCloseReason>),
    /// Pinned TLS or key proof failed.
    NativeHandshake,
    /// Gateway refused the native product selector.
    ProductRefused,
    /// Authentication answered with its typed close reason.
    Authentication(SessionCloseReason),
    /// Malformed or contradictory protocol frame.
    Protocol,
    /// Reply did not name the original in-flight ticket.
    Correlation,
    /// The physical worker panicked or could not start.
    WorkerPanicked,
    /// Opening has not produced an actual Ready.
    NotReady,
}
impl ProductFailure {
    /// The lifecycle class when this failure is a close. A protocol failure,
    /// including a malformed `session.terminated`, is not a class.
    pub fn connection_outcome(self) -> Option<ConnectionOutcome> {
        match self {
            Self::Closed(reason) => Some(connection_outcome(reason)),
            Self::Authentication(reason) => Some(connection_outcome(Some(reason))),
            _ => None,
        }
    }
}
impl From<GatewayError> for ProductFailure {
    fn from(error: GatewayError) -> Self {
        match error {
            GatewayError::InvalidPolicy => Self::InvalidLimits,
            GatewayError::InvalidRequest | GatewayError::InvalidCredential => Self::InvalidRequest,
            GatewayError::DriverPanicked => Self::WorkerPanicked,
            GatewayError::Cancelled => Self::Cancelled,
            GatewayError::TimedOut => Self::TimedOut,
            GatewayError::Transport => Self::Transport,
            GatewayError::Closed(reason) => Self::Closed(reason),
            GatewayError::Correlation => Self::Correlation,
            GatewayError::NativeHandshake => Self::NativeHandshake,
            GatewayError::ResponseTooLarge => Self::ResponseTooLarge,
            GatewayError::RequestTooLarge => Self::RequestTooLarge,
            GatewayError::EventCapacity | GatewayError::Busy => Self::Capacity,
            GatewayError::ProductRefused => Self::ProductRefused,
            GatewayError::Authentication(reason) => Self::Authentication(reason),
            _ => Self::Protocol,
        }
    }
}
/// Original request settlement. Gateway success and error retain their complete
/// response; local failure distinguishes proven absence of dispatch from uncertainty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProductCompletion {
    /// Exact correlated factual gateway reply.
    Response(Box<ResponseFrame>),
    /// The worker did not record dispatch for this ticket.
    NotDispatched(ProductFailure),
    /// Dispatch was recorded before the first write; outcome is unconfirmed.
    Uncertain(ProductFailure),
}
/// Immutable injected operational capacities; wire bounds remain protocol-owned.
#[derive(Clone, Copy, Debug)]
pub struct ProductLimits {
    pub(super) pending: usize,
    pub(super) events: usize,
    pub(super) event_bytes: usize,
    pub(super) poll_ms: u64,
    pub(super) request_ms: u64,
    pub(super) handshake_ms: u64,
    pub(super) frame_ms: u64,
}
impl ProductLimits {
    /// Checked local policy. `event_bytes` may be smaller than the published
    /// per-frame bound, so any arriving event still checks both budgets.
    pub fn new(
        pending: usize,
        events: usize,
        event_bytes: usize,
        poll_ms: u64,
        request_ms: u64,
        handshake_ms: u64,
        frame_ms: u64,
    ) -> Result<Self, ProductFailure> {
        if pending == 0
            || events == 0
            || event_bytes == 0
            || poll_ms == 0
            || request_ms == 0
            || handshake_ms == 0
            || frame_ms == 0
            || events.checked_mul(MAX_PROTECTED_RESPONSE_BYTES).is_none()
            || pending
                .checked_mul(MAX_PROTECTED_RESPONSE_BYTES.max(MAX_PROTECTED_REQUEST_BYTES))
                .is_none()
        {
            return Err(ProductFailure::InvalidLimits);
        }
        Ok(Self {
            pending,
            events,
            event_bytes,
            poll_ms,
            request_ms,
            handshake_ms,
            frame_ms,
        })
    }
}
impl Default for ProductLimits {
    fn default() -> Self {
        Self::new(
            8,
            32,
            32 * MAX_PROTECTED_RESPONSE_BYTES,
            100,
            30_000,
            30_000,
            30_000,
        )
        .expect("fixed operational defaults fit")
    }
}
pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
pub(super) struct State {
    pub(super) ready: Option<Arc<ProductSessionReady>>,
    pub(super) failure: Option<ProductFailure>,
    pub(super) closing: bool,
    pub(super) drained: bool,
    pub(super) interrupt: Option<Arc<dyn ProductInterrupt>>,
    pub(super) next_id: u64,
    pub(super) permits: usize,
    pub(super) queue: VecDeque<Arc<Ticket>>,
    pub(super) active: Option<(Arc<Ticket>, bool)>,
    pub(super) events: VecDeque<(EventFrame, usize)>,
    pub(super) event_bytes: usize,
    pub(super) event_count: usize,
}
pub(super) struct Owner {
    pub(super) state: Mutex<State>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) wake: Arc<dyn ProductWake>,
    pub(super) limits: ProductLimits,
}
impl Owner {
    pub(super) fn record(&self, cause: ProductFailure) {
        {
            let mut state = lock(&self.state);
            state.failure.get_or_insert(cause);
            state.closing = true;
        }
        self.notify();
    }
    /// The wake port is injected. A panic there must not unwind after admission
    /// or cleanup has been committed: the caller still receives its ticket, and
    /// the session failure is `WorkerPanicked` unless a decoded failure was
    /// already recorded. This does not wake again.
    pub(super) fn notify(&self) {
        let panicked = catch_unwind(AssertUnwindSafe(|| self.wake.wake())).is_err();
        if !panicked {
            return;
        }
        let mut state = lock(&self.state);
        state.closing = true;
        if matches!(state.failure, None | Some(ProductFailure::Cancelled)) {
            state.failure = Some(ProductFailure::WorkerPanicked);
        }
    }
    pub(super) fn stop(&self) {
        let interrupt = {
            let mut state = lock(&self.state);
            state.closing = true;
            state.failure.get_or_insert(ProductFailure::Cancelled);
            state.interrupt.clone()
        };
        // The interrupt is injected. A panic there must not skip the wake or
        // the join in `close`: the worker is still drained, and the failure
        // is `WorkerPanicked` unless a decoded failure was already recorded.
        if let Some(interrupt) = interrupt {
            let panicked = catch_unwind(AssertUnwindSafe(|| interrupt.interrupt())).is_err();
            if panicked {
                let mut state = lock(&self.state);
                if matches!(state.failure, None | Some(ProductFailure::Cancelled)) {
                    state.failure = Some(ProductFailure::WorkerPanicked);
                }
            }
        }
        self.notify();
    }
    pub(super) fn finish(&self, cause: ProductFailure) {
        let (queue, active, cause) = {
            let mut state = lock(&self.state);
            state.failure.get_or_insert(cause);
            state.closing = true;
            state.drained = true;
            state.interrupt.take();
            let cause = *state.failure.get_or_insert(cause);
            (
                state.queue.drain(..).collect::<Vec<_>>(),
                state.active.take(),
                cause,
            )
        };
        if let Some((ticket, dispatched)) = active {
            ticket.complete(if dispatched {
                ProductCompletion::Uncertain(cause)
            } else {
                ProductCompletion::NotDispatched(cause)
            });
        }
        for ticket in queue {
            ticket.complete(ProductCompletion::NotDispatched(cause));
        }
        self.notify();
    }
}
enum Worker {
    Prepared(Open),
    Running(JoinHandle<()>),
    Joined,
}
struct Handle {
    owner: Arc<Owner>,
    worker: Mutex<Worker>,
}
impl Handle {
    fn close(&self) -> Result<(), ProductFailure> {
        self.owner.stop();
        let mut worker = lock(&self.worker);
        match std::mem::replace(&mut *worker, Worker::Joined) {
            Worker::Running(thread) => {
                if thread.join().is_err() {
                    self.owner.finish(ProductFailure::WorkerPanicked);
                }
            }
            Worker::Prepared(_) => self.owner.finish(ProductFailure::Cancelled),
            Worker::Joined => {}
        }
        let cause = lock(&self.owner.state).failure;
        match cause {
            None | Some(ProductFailure::Cancelled) => Ok(()),
            Some(cause) => Err(cause),
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
/// Opaque physical product child. `prepare` performs no connect, so its parent
/// can register the child before calling `start`. Last-handle drop joins the
/// worker; foreign hosts must arrange that release off their UI queue.
#[derive(Clone)]
pub struct ProductSession {
    handle: Arc<Handle>,
}
impl ProductSession {
    /// Capture immutable original inputs without network effects. The saved key
    /// remains private and owned through actual worker drain.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        address: SocketAddr,
        identity: Arc<NativeIdentity>,
        pin: [u8; 44],
        credential: String,
        instance: String,
        connector: Arc<dyn ProductConnector>,
        clock: Arc<dyn Clock>,
        wake: Arc<dyn ProductWake>,
        limits: ProductLimits,
    ) -> Self {
        let owner = Arc::new(Owner {
            state: Mutex::new(State {
                ready: None,
                failure: None,
                closing: false,
                drained: false,
                interrupt: None,
                next_id: 1,
                permits: 0,
                queue: VecDeque::new(),
                active: None,
                events: VecDeque::new(),
                event_bytes: 0,
                event_count: 0,
            }),
            clock,
            wake,
            limits,
        });
        let open = Open {
            address,
            identity,
            pin,
            credential,
            instance,
            connector,
        };
        Self {
            handle: Arc::new(Handle {
                owner,
                worker: Mutex::new(Worker::Prepared(open)),
            }),
        }
    }
    /// Begin the one physical opening after the caller registers ownership.
    pub fn start(&self) -> Result<(), ProductFailure> {
        let mut slot = lock(&self.handle.worker);
        if lock(&self.handle.owner.state).closing {
            return Err(ProductFailure::Cancelled);
        }
        if !matches!(*slot, Worker::Prepared(_)) {
            return Err(ProductFailure::InvalidRequest);
        }
        let Worker::Prepared(open) = std::mem::replace(&mut *slot, Worker::Joined) else {
            unreachable!()
        };
        let owner = self.handle.owner.clone();
        match thread::Builder::new()
            .name("nessa-product".into())
            .spawn(move || worker::run(owner, open))
        {
            Ok(thread) => {
                *slot = Worker::Running(thread);
                Ok(())
            }
            Err(_) => {
                self.handle.owner.finish(ProductFailure::WorkerPanicked);
                Err(ProductFailure::WorkerPanicked)
            }
        }
    }
    /// Actual immutable ready metadata, preserved historically after close.
    pub fn ready(&self) -> Option<Arc<ProductSessionReady>> {
        lock(&self.handle.owner.state).ready.clone()
    }
    /// First terminal physical cause, distinct from retained factual Ready.
    pub fn failure(&self) -> Option<ProductFailure> {
        lock(&self.handle.owner.state).failure
    }
    /// Wait for actual opening settlement. Cancelling the caller does not stop it.
    pub fn wait_ready(&self) -> Result<Arc<ProductSessionReady>, ProductFailure> {
        let owner = &self.handle.owner;
        loop {
            let revision = owner.wake.revision();
            {
                let state = lock(&owner.state);
                if let Some(ready) = &state.ready {
                    return Ok(ready.clone());
                }
                if let Some(cause) = state.failure {
                    return Err(cause);
                }
                if state.closing {
                    return Err(ProductFailure::Cancelled);
                }
            }
            owner
                .wake
                .wait(revision, Duration::from_millis(owner.limits.poll_ms));
        }
    }
    /// Admit one original bounded request. Capacity covers queued, dispatched and
    /// completed-but-unreleased tickets (`Ticket::complete` and ticket Drop).
    pub fn request(
        &self,
        method: &str,
        params_json: &str,
    ) -> Result<ProductTicket, ProductFailure> {
        let owner = &self.handle.owner;
        let mut state = lock(&owner.state);
        if state.closing {
            return Err(state.failure.unwrap_or(ProductFailure::Cancelled));
        }
        let ready = state.ready.as_ref().ok_or(ProductFailure::NotReady)?;
        if !ready.methods.iter().any(|advertised| advertised == method) {
            return Err(ProductFailure::UnadvertisedMethod);
        }
        if state.permits >= owner.limits.pending {
            return Err(ProductFailure::Capacity);
        }
        if params_json.len() > MAX_PROTECTED_REQUEST_BYTES {
            return Err(ProductFailure::RequestTooLarge);
        }
        let params = unique_value(params_json).map_err(|_| ProductFailure::InvalidRequest)?;
        let id = state.next_id;
        let next = id.checked_add(1).ok_or(ProductFailure::Capacity)?;
        let text =
            encode_request(&id.to_string(), method, &params).map_err(|error| match error {
                ReadEncodeError::ResponseTooLarge => ProductFailure::RequestTooLarge,
                ReadEncodeError::InvalidPayload => ProductFailure::InvalidRequest,
            })?;
        if text.len() > MAX_PROTECTED_REQUEST_BYTES {
            return Err(ProductFailure::RequestTooLarge);
        }
        // Consume the protocol's unique-key/value-budget decoder as outer owner.
        RequestFrame::decode(&text).map_err(|_| ProductFailure::InvalidRequest)?;
        let deadline = owner
            .clock
            .elapsed_ms()
            .checked_add(owner.limits.request_ms)
            .ok_or(ProductFailure::TimedOut)?;
        let ticket = Arc::new(Ticket {
            id,
            text,
            deadline,
            owner: Arc::downgrade(owner),
            state: Mutex::new(TicketState {
                external: 1,
                completion: None,
            }),
        });
        state.next_id = next;
        state.permits += 1;
        state.queue.push_back(ticket.clone());
        drop(state);
        owner.notify();
        Ok(ProductTicket { ticket })
    }
    /// Take an event batch bounded by the session's count and byte capacities.
    /// Waits until events, the caller's absolute deadline, or terminal close.
    pub fn events(&self, deadline_ms: u64) -> Result<ProductEventBatch, ProductFailure> {
        let owner = &self.handle.owner;
        loop {
            let revision = owner.wake.revision();
            {
                let mut state = lock(&owner.state);
                if !state.events.is_empty() {
                    let mut bytes = 0;
                    let events = state
                        .events
                        .drain(..)
                        .map(|(event, size)| {
                            bytes += size;
                            event
                        })
                        .collect();
                    return Ok(ProductEventBatch {
                        inner: Arc::new(EventBatch {
                            owner: Arc::downgrade(owner),
                            events,
                            bytes,
                        }),
                    });
                }
                if let Some(cause) = state.failure {
                    return Err(cause);
                }
                if state.closing {
                    return Err(ProductFailure::Cancelled);
                }
            }
            let left = deadline_ms.saturating_sub(owner.clock.elapsed_ms());
            if left == 0 {
                return Ok(ProductEventBatch {
                    inner: Arc::new(EventBatch {
                        owner: Arc::downgrade(owner),
                        events: Vec::new(),
                        bytes: 0,
                    }),
                });
            }
            owner.wake.wait(
                revision,
                Duration::from_millis(left.min(owner.limits.poll_ms)),
            );
        }
    }
    /// Fence admission and independently interrupt blocked native I/O.
    pub fn request_close(&self) {
        self.handle.owner.stop();
    }
    /// Join the original physical owner. Repeated calls keep its first failure.
    pub fn close(&self) -> Result<(), ProductFailure> {
        self.handle.close()
    }
}
struct TicketState {
    external: usize,
    completion: Option<Arc<ProductCompletion>>,
}
pub(super) struct Ticket {
    pub(super) id: u64,
    pub(super) text: String,
    pub(super) deadline: u64,
    owner: Weak<Owner>,
    state: Mutex<TicketState>,
}
impl Ticket {
    pub(super) fn complete(&self, completion: ProductCompletion) {
        let mut state = lock(&self.state);
        if state.completion.is_some() {
            return;
        }
        state.completion = Some(Arc::new(completion));
        if let Some(owner) = self.owner.upgrade() {
            if state.external == 0 {
                lock(&owner.state).permits -= 1;
            }
            owner.notify();
        }
    }
}
/// Immutable original wire ticket; dropping an observer does not cancel dispatch.
pub struct ProductTicket {
    ticket: Arc<Ticket>,
}
impl ProductTicket {
    /// Native-allocated wire correlation ID after reserved handshake ID0.
    pub fn id(&self) -> u64 {
        self.ticket.id
    }
    /// Factual settlement when available.
    pub fn completion(&self) -> Option<ProductSettlement> {
        let completion = lock(&self.ticket.state).completion.clone()?;
        Some(ProductSettlement {
            ticket: self.clone(),
            completion,
        })
    }
    /// Wait for settlement independently of observer cancellation.
    pub fn wait(&self) -> ProductSettlement {
        loop {
            if let Some(owner) = self.ticket.owner.upgrade() {
                let revision = owner.wake.revision();
                if let Some(completion) = self.completion() {
                    return completion;
                }
                owner
                    .wake
                    .wait(revision, Duration::from_millis(owner.limits.poll_ms));
            } else {
                return self
                    .completion()
                    .expect("last session joined original tickets");
            }
        }
    }
}
impl Clone for ProductTicket {
    fn clone(&self) -> Self {
        lock(&self.ticket.state).external += 1;
        Self {
            ticket: self.ticket.clone(),
        }
    }
}
impl Drop for ProductTicket {
    fn drop(&mut self) {
        let mut state = lock(&self.ticket.state);
        state.external -= 1;
        if state.external == 0 && state.completion.is_some() {
            if let Some(owner) = self.ticket.owner.upgrade() {
                lock(&owner.state).permits -= 1;
                owner.notify();
            }
        }
    }
}

/// Borrowed factual completion retaining the ticket's native capacity permit.
/// Keeping a settlement after dropping its original ticket still occupies that
/// ticket until physical completion and this last observer release.
pub struct ProductSettlement {
    ticket: ProductTicket,
    completion: Arc<ProductCompletion>,
}
impl ProductSettlement {
    /// Original native correlation identity.
    pub fn id(&self) -> u64 {
        self.ticket.id()
    }
    /// Original typed response/failure; this accessor does not copy its body.
    pub fn value(&self) -> &ProductCompletion {
        &self.completion
    }
}

struct EventBatch {
    owner: Weak<Owner>,
    events: Vec<EventFrame>,
    bytes: usize,
}
impl Drop for EventBatch {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut state = lock(&owner.state);
            state.event_count -= self.events.len();
            state.event_bytes -= self.bytes;
            drop(state);
            owner.notify();
        }
    }
}
/// Immutable bounded delivered event batch. Copies of this handle share its
/// native bodies and retain count/byte permits until the last handle release.
#[derive(Clone)]
pub struct ProductEventBatch {
    inner: Arc<EventBatch>,
}
impl ProductEventBatch {
    /// Original ordered parsed frames; deliberate caller copies are history.
    pub fn values(&self) -> &[EventFrame] {
        &self.inner.events
    }
}
impl Deref for ProductEventBatch {
    type Target = [EventFrame];
    fn deref(&self) -> &Self::Target {
        self.values()
    }
}
