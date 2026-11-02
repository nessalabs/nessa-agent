//! One SDK physical operation executes outside Tokio enter and joins its source on drop.
use super::source::ReadSelection;
use crate::conversation::application::{
    RecordHead, RecordReadError, RecordReadOperation, RecordReadValue,
};
use nessa_sdk::{
    application::agent_execution::sessions::StorageError,
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::{RecordReadStatus, RecordStorage, RecordStreamIdentity},
};
use nessa_sync::replication::{application::SourceError, domain::Scope};
use std::{sync::Arc, time::Duration};
use tokio::runtime::Handle;

/// SDK discovery calls one admitted read may make before it answers
/// `source_preparing`. Each call is one bounded SDK step. Bounds and
/// orderings: "Steps per admitted read" in
/// `docs/design/bounded-terminal-discovery.md`.
pub(crate) const DISCOVERY_STEPS_PER_READ: usize = 128;
/// How long one admitted read keeps making discovery calls while it holds a
/// global read permit and its socket's record slot (row S5). The clock starts
/// when the first discovery call is about to run, after the read has a worker
/// and has looked up the stream. Time spent waiting for that worker is not
/// discovery.
pub(crate) const READ_WORK_BUDGET: Duration = Duration::from_millis(200);

/// One physical operation and when its discovery calls must stop.
pub(super) struct ReadWork {
    pub(super) operation: RecordReadOperation,
    /// Most SDK discovery calls this read makes.
    pub(super) steps: usize,
    /// Asked before every call after the first: true once the read's budget
    /// has passed, its waiter is gone, or read worker shutdown has started.
    /// A test may hold the worker inside this callback until that is true.
    pub(super) stopped: Box<dyn Fn() -> bool + Send>,
    /// The same end, without a test hold. An occupied cache waits on this
    /// so the hold at a real step boundary is not also the wait for an owner.
    pub(super) budget_elapsed: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub(super) fn execute(
    storage: Arc<RecordStorage>,
    runtime: Handle,
    session: SessionId,
    identity: RecordStreamIdentity,
    admitted: ReadSelection,
    observed: Scope,
    work: ReadWork,
) -> Result<RecordReadValue, RecordReadError> {
    let mut source = runtime
        .block_on(storage.record_source_expected(&session, &identity))
        .map_err(storage_error)?;
    admitted
        .check_opened(&source, &observed)
        .map_err(RecordReadError::Admission)?;
    let ReadWork {
        operation,
        steps,
        stopped,
        budget_elapsed,
    } = work;
    let wait_storage = storage.clone();
    let wait_budget = Arc::clone(&budget_elapsed);
    let value = match operation {
        RecordReadOperation::Head => discover(
            steps,
            &*stopped,
            &*budget_elapsed,
            || source.bounded_head(&observed),
            || wait_storage.wait_for_discovery_release(&*wait_budget),
        )
        .map(|head| {
            RecordReadValue::Head(RecordHead {
                scope: observed,
                head,
            })
        }),
        RecordReadOperation::Page(page) => {
            let wait_storage = storage.clone();
            let wait_budget = Arc::clone(&budget_elapsed);
            discover(
                steps,
                &*stopped,
                &*budget_elapsed,
                || source.bounded_page(&page),
                || wait_storage.wait_for_discovery_release(&*wait_budget),
            )
            .map(RecordReadValue::Page)
        }
    };
    // The final SDK source drop joins its internal worker
    // here because this thread is outside a Tokio enter.
    drop(source);
    value
}

/// Ask the SDK again while it is still validating, up to `steps` calls or until
/// `stopped`. The first call is always made, so every read advances; the SDK
/// keeps every step's progress, so a later read resumes where this one stopped.
/// The first refusal ends the read.
///
/// [`RecordReadStatus::Occupied`] did no validation. It does not count as one
/// of `steps`. `wait_for_owner` blocks until a checked-out entry comes back
/// or the read's budget elapses; `budget_elapsed` is that flag without the
/// step-boundary test hold `stopped` may include.
fn discover<T>(
    steps: usize,
    stopped: &dyn Fn() -> bool,
    budget_elapsed: &dyn Fn() -> bool,
    mut call: impl FnMut() -> Result<RecordReadStatus<T>, SourceError>,
    mut wait_for_owner: impl FnMut(),
) -> Result<T, RecordReadError> {
    let mut exhausted = true;
    let mut step = 0;
    loop {
        if step > 0 && stopped() {
            exhausted = false;
            break;
        }
        if step >= steps {
            break;
        }
        match call().map_err(source_error)? {
            RecordReadStatus::Ready(value) => return Ok(value),
            RecordReadStatus::Preparing => step += 1,
            RecordReadStatus::Occupied if budget_elapsed() => {
                exhausted = false;
                break;
            }
            RecordReadStatus::Occupied => wait_for_owner(),
        }
    }
    // The step loop ended because it ran out, not because the read was asked
    // to stop. Stopping at the budget is the caller's to name.
    if exhausted {
        crate::core::limit_log::note_limit("record.discovery_steps");
    }
    Err(RecordReadError::SourcePreparing)
}

pub(super) fn storage_error(error: StorageError) -> RecordReadError {
    match error {
        StorageError::IdentityMismatch => RecordReadError::IdentityChanged,
        // `Store` is the records database. `Corrupt` and `AnotherVersion` are
        // this conversation's bytes. This read does not open the chat, so
        // every one of them stays temporarily unavailable.
        _ => RecordReadError::TemporarilyUnavailable,
    }
}

fn source_error(error: SourceError) -> RecordReadError {
    match error {
        SourceError::Unavailable => RecordReadError::TemporarilyUnavailable,
        SourceError::Pruned => RecordReadError::HistoryPruned,
        SourceError::InvalidRequest => RecordReadError::InvalidRequest,
        SourceError::IdentityChanged => RecordReadError::IdentityChanged,
        SourceError::OversizedRecord => RecordReadError::RecordTooLarge,
    }
}

#[cfg(test)]
mod discover_steps {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn occupied_answers_do_not_spend_discovery_steps() {
        let calls = AtomicUsize::new(0);
        let waits = AtomicUsize::new(0);
        let found = discover(
            2,
            &|| false,
            &|| false,
            || {
                let call = calls.fetch_add(1, Ordering::SeqCst);
                if call < 5 {
                    Ok(RecordReadStatus::Occupied)
                } else {
                    Ok(RecordReadStatus::Ready(7_u64))
                }
            },
            || {
                waits.fetch_add(1, Ordering::SeqCst);
            },
        )
        .expect("the read reaches the head");
        assert_eq!(found, 7);
        assert_eq!(waits.load(Ordering::SeqCst), 5);
        assert_eq!(calls.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn an_elapsed_budget_stops_on_occupied_without_burning_the_step_count() {
        let calls = AtomicUsize::new(0);
        let error = discover(
            128,
            &|| true,
            &|| true,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(RecordReadStatus::<u64>::Occupied)
            },
            || panic!("the budget has elapsed; the read must not wait"),
        )
        .expect_err("still occupied when the budget elapses");
        assert!(matches!(error, RecordReadError::SourcePreparing));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_real_preparing_answer_still_spends_one_step() {
        let calls = AtomicUsize::new(0);
        let error = discover(
            2,
            &|| false,
            &|| false,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(RecordReadStatus::<u64>::Preparing)
            },
            || panic!("a validation step is not an occupied cache"),
        )
        .expect_err("two steps are the whole budget");
        assert!(matches!(error, RecordReadError::SourcePreparing));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
