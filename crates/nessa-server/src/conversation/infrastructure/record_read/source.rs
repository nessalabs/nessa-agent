//! SDK identity and physical reads on tracked threads after passive admission.

use super::operation;
use crate::conversation::application::conversation_session;
use crate::conversation::application::{
    RecordReadError, RecordReadFuture, RecordReadLease, RecordReadOperation, RecordReadResponse,
    RecordReadSource,
};
use crate::conversation::infrastructure::record_scope_from_identity;
use crate::core::read_workers::{ReadWorkerError, ReadWorkers};
use nessa_protocol::conversation::read_scope::{ReadRefusal, ReceiverReadScope};
use nessa_sdk::{
    domain::agent_execution::sessions::SessionId, infrastructure::session_storage::RecordStorage,
};
use nessa_sync::replication::domain::Id;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(test)]
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
#[cfg(test)]
use tokio::sync::Notify;

/// A cold-read budget that cannot be a deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidReadWorkBudget;

impl std::fmt::Display for InvalidReadWorkBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "read work budget must be positive and representable as a deadline"
        )
    }
}

impl std::error::Error for InvalidReadWorkBudget {}

/// The deadline rule `OperationalLimits` uses for this budget. Product is not
/// imported here, so the source checks the duration it is given itself.
fn accept_read_work_budget(work_budget: Duration) -> Result<Duration, InvalidReadWorkBudget> {
    if work_budget.is_zero() || Instant::now().checked_add(work_budget).is_none() {
        Err(InvalidReadWorkBudget)
    } else {
        Ok(work_budget)
    }
}

/// One physical SDK source per read; no Agent or writer lease is opened.
pub struct NessaRecordReadSource {
    storage: Arc<RecordStorage>,
    origin: Id,
    runtime: Handle,
    workers: Arc<ReadWorkers>,
    discovery_steps: usize,
    work_budget: Duration,
    #[cfg(test)]
    before_identity: Option<Arc<TestReadGate>>,
    /// Called with the stop condition at each step boundary; a test may hold
    /// the worker there until the condition it is driving has been set.
    #[cfg(test)]
    between_steps: Option<BetweenSteps>,
}

/// A test hold at each step boundary, given the read's stop condition.
#[cfg(test)]
type BetweenSteps = Arc<dyn Fn(&dyn Fn() -> bool) + Send + Sync>;

#[cfg(test)]
pub(crate) struct TestReadGate {
    entered: Notify,
    panic_after_release: bool,
    open: Mutex<bool>,
    released: Condvar,
}

#[cfg(test)]
impl TestReadGate {
    pub(crate) async fn entered(&self) {
        self.entered.notified().await;
    }
    fn wait(&self) {
        self.entered.notify_one();
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.released.wait(open).unwrap();
        }
        // Model physical work failure without poisoning the fixture release gate.
        drop(open);
        assert!(!self.panic_after_release, "injected read worker panic");
    }

    pub(crate) fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.released.notify_all();
    }
}

impl NessaRecordReadSource {
    pub fn new(storage: Arc<RecordStorage>, origin: Id, runtime: Handle) -> Self {
        Self {
            storage,
            origin,
            runtime,
            workers: ReadWorkers::new(),
            discovery_steps: operation::DISCOVERY_STEPS_PER_READ,
            work_budget: operation::READ_WORK_BUDGET,
            #[cfg(test)]
            before_identity: None,
            #[cfg(test)]
            between_steps: None,
        }
    }

    /// How long this source's cold reads may look. Composition sets it from
    /// the gateway's operational limits; tests keep [`Self::new`]'s default.
    /// A zero budget, or one that cannot be a deadline, is refused: a zero
    /// sleep would win every cold read before discovery moved.
    pub fn with_work_budget(
        mut self,
        work_budget: Duration,
    ) -> Result<Self, InvalidReadWorkBudget> {
        self.work_budget = accept_read_work_budget(work_budget)?;
        Ok(self)
    }

    #[cfg(test)]
    pub(crate) fn held_for_host_shutdown(
        storage: Arc<RecordStorage>,
        origin: Id,
        runtime: Handle,
        panic_after_release: bool,
    ) -> (Self, Arc<TestReadGate>) {
        let gate = Arc::new(TestReadGate {
            entered: Notify::new(),
            panic_after_release,
            open: Mutex::new(false),
            released: Condvar::new(),
        });
        let mut source = Self::new(storage, origin, runtime);
        source.before_identity = Some(gate.clone());
        (source, gate)
    }

    /// Fence new reads, then join identity lookup and physical source work before
    /// storage shutdown. Cancelled waiters retain the same blocking join.
    pub async fn shutdown(&self) -> Result<(), RecordReadError> {
        self.workers.shutdown().await.map_err(worker_error)
    }
}

impl RecordReadSource for NessaRecordReadSource {
    fn read<'a>(
        &'a self,
        admitted: ReceiverReadScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse> {
        Box::pin(async move {
            self.workers.admit().map_err(worker_error)?;
            let session = session_id(&admitted)?;
            let storage = self.storage.clone();
            let runtime = self.runtime.clone();
            let origin = self.origin.clone();
            let steps = self.discovery_steps;
            // Set when the budget passes or this waiter is dropped (the product
            // deadline answered read_timeout). A disconnected socket detaches
            // its read task instead, so that read stops at the budget.
            let stop = Arc::new(AtomicBool::new(false));
            let stopped: Box<dyn Fn() -> bool + Send> = {
                let stop = stop.clone();
                let workers = self.workers.clone();
                Box::new(move || stop.load(Ordering::SeqCst) || workers.is_closed())
            };
            #[cfg(test)]
            let stopped: Box<dyn Fn() -> bool + Send> = match self.between_steps.clone() {
                Some(hold) => Box::new(move || {
                    hold(&*stopped);
                    stopped()
                }),
                None => stopped,
            };
            #[cfg(test)]
            let before_identity = self.before_identity.clone();
            let work = self.workers.run("nessa-record-read", move || {
                debug_assert!(Handle::try_current().is_err());
                #[cfg(test)]
                if let Some(gate) = before_identity {
                    gate.wait();
                }
                let identity = runtime
                    .block_on(storage.record_identity(&session, origin))
                    .map_err(operation::storage_error)?
                    .ok_or(RecordReadError::IdentityChanged)?;
                let observed = record_scope_from_identity(&admitted, &identity)
                    .map_err(RecordReadError::Admission)?;
                if let RecordReadOperation::Page(request) = &operation {
                    if request.scope != observed {
                        return Err(RecordReadError::Admission(ReadRefusal::Unverifiable));
                    }
                }
                let work = operation::ReadWork {
                    operation,
                    steps,
                    stopped,
                };
                let result = operation::execute(
                    storage, runtime, session, identity, admitted, observed, work,
                );
                result.map(|value| RecordReadResponse { value, lease })
            });
            let mut work = std::pin::pin!(work);
            // Declared after `work`, so it is dropped first: a dropped waiter
            // publishes `stop` before the worker's answer channel is released.
            let _waiter = StopWhenDropped(stop.clone());
            let finished = tokio::select! {
                biased;
                finished = &mut work => finished,
                () = tokio::time::sleep(self.work_budget) => {
                    // The worker stops at its next step boundary.
                    stop.store(true, Ordering::SeqCst);
                    let finished = work.await;
                    // The budget won, and the read's own answer is that the
                    // source is still preparing. A worker failure is not this limit.
                    if matches!(finished, Ok(Err(RecordReadError::SourcePreparing))) {
                        crate::core::limit_log::note_limit("record.read_work_budget");
                    }
                    finished
                }
            };
            finished.map_err(worker_error)?
        })
    }
}

/// Stops the read's discovery calls once nobody waits for its answer.
struct StopWhenDropped(Arc<AtomicBool>);

impl Drop for StopWhenDropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn worker_error(error: ReadWorkerError) -> RecordReadError {
    match error {
        ReadWorkerError::Unavailable => RecordReadError::TemporarilyUnavailable,
        ReadWorkerError::WorkerPanicked => RecordReadError::WorkerPanicked,
    }
}

fn session_id(admitted: &ReceiverReadScope) -> Result<SessionId, RecordReadError> {
    Ok(conversation_session(&admitted.conversation_id))
}

#[cfg(test)]
mod work_budget {
    use std::sync::Arc;
    use std::time::Duration;

    use nessa_sdk::infrastructure::session_storage::RecordStorage;
    use nessa_sync::replication::domain::Id;
    use tokio::runtime::Handle;

    use super::{accept_read_work_budget, NessaRecordReadSource};

    #[test]
    fn a_budget_that_cannot_be_a_deadline_is_refused() {
        assert!(accept_read_work_budget(Duration::ZERO).is_err());
        assert!(accept_read_work_budget(Duration::MAX).is_err());
        assert_eq!(
            accept_read_work_budget(Duration::from_millis(200)).unwrap(),
            Duration::from_millis(200)
        );
    }

    #[tokio::test]
    async fn the_builder_refuses_a_budget_that_cannot_be_a_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
        let zero = NessaRecordReadSource::new(
            storage.clone(),
            Id::new("origin").unwrap(),
            Handle::current(),
        );
        let unbounded = NessaRecordReadSource::new(
            storage.clone(),
            Id::new("origin").unwrap(),
            Handle::current(),
        );
        let accepted =
            NessaRecordReadSource::new(storage, Id::new("origin").unwrap(), Handle::current());
        assert!(zero.with_work_budget(Duration::ZERO).is_err());
        assert!(unbounded.with_work_budget(Duration::MAX).is_err());
        assert!(accepted
            .with_work_budget(Duration::from_millis(200))
            .is_ok());
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/record_read/source.rs"]
mod tests;
