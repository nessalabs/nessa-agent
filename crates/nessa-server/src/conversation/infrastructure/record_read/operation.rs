//! One SDK physical operation executes outside Tokio enter and joins its source on drop.
use crate::conversation::{
    application::{RecordHead, RecordReadError, RecordReadOperation, RecordReadValue},
    infrastructure::exact_record_scope,
};
use nessa_protocol::conversation::read_scope::ReceiverReadScope;
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
/// global read permit and its socket's record slot (row S5).
pub(crate) const READ_WORK_BUDGET: Duration = Duration::from_millis(200);

/// One physical operation and when its discovery calls must stop.
pub(super) struct ReadWork {
    pub(super) operation: RecordReadOperation,
    /// Most SDK discovery calls this read makes.
    pub(super) steps: usize,
    /// Asked before every call after the first: true once the read's budget
    /// has passed, its waiter is gone, or read worker shutdown has started.
    pub(super) stopped: Box<dyn Fn() -> bool + Send>,
}

pub(super) fn execute(
    storage: Arc<RecordStorage>,
    runtime: Handle,
    session: SessionId,
    identity: RecordStreamIdentity,
    admitted: ReceiverReadScope,
    observed: Scope,
    work: ReadWork,
) -> Result<RecordReadValue, RecordReadError> {
    let mut source = runtime
        .block_on(storage.record_source_expected(&session, &identity))
        .map_err(storage_error)?;
    exact_record_scope(&admitted, &source, &observed).map_err(RecordReadError::Admission)?;
    let ReadWork {
        operation,
        steps,
        stopped,
    } = work;
    let value = match operation {
        RecordReadOperation::Head => discover(steps, &*stopped, || source.bounded_head(&observed))
            .map(|head| {
                RecordReadValue::Head(RecordHead {
                    scope: observed,
                    head,
                })
            }),
        RecordReadOperation::Page(page) => {
            discover(steps, &*stopped, || source.bounded_page(&page)).map(RecordReadValue::Page)
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
fn discover<T>(
    steps: usize,
    stopped: &dyn Fn() -> bool,
    mut call: impl FnMut() -> Result<RecordReadStatus<T>, SourceError>,
) -> Result<T, RecordReadError> {
    let mut exhausted = true;
    for step in 0..steps {
        if step > 0 && stopped() {
            exhausted = false;
            break;
        }
        if let RecordReadStatus::Ready(value) = call().map_err(source_error)? {
            return Ok(value);
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
        StorageError::Corrupt(_) | StorageError::AnotherVersion { .. } => {
            RecordReadError::Unreadable
        }
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
mod tests {
    use super::*;

    #[test]
    fn corrupt_and_another_version_are_permanent() {
        assert_eq!(
            storage_error(StorageError::Corrupt("body".into())),
            RecordReadError::Unreadable
        );
        assert_eq!(
            storage_error(StorageError::AnotherVersion { found: None }),
            RecordReadError::Unreadable
        );
        assert_eq!(
            storage_error(StorageError::AnotherVersion { found: Some(2) }),
            RecordReadError::Unreadable
        );
        assert_eq!(
            storage_error(StorageError::Io("disk".into())),
            RecordReadError::TemporarilyUnavailable
        );
    }
}
