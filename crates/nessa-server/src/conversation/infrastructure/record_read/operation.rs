//! One SDK physical operation executes outside Tokio enter and joins its source on drop.
use crate::conversation::{
    application::{
        ReceiverReadScope, RecordHead, RecordReadError, RecordReadOperation, RecordReadValue,
    },
    infrastructure::exact_record_scope,
};
use nessa_sdk::{
    application::agent_execution::sessions::StorageError,
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::{RecordReadStatus, RecordStorage, RecordStreamIdentity},
};
use nessa_sync::replication::{application::SourceError, domain::Scope};
use std::sync::Arc;
use tokio::runtime::Handle;

/// SDK discovery calls one admitted read may make before it answers
/// `source_preparing`. Each call is one bounded SDK step; this is the whole of
/// the adapter's decision. Bounds and orderings: "Steps per admitted read" in
/// `docs/design/bounded-terminal-discovery.md`.
pub(super) const DISCOVERY_STEPS_PER_READ: usize = 128;

#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    storage: Arc<RecordStorage>,
    runtime: Handle,
    session: SessionId,
    identity: RecordStreamIdentity,
    admitted: ReceiverReadScope,
    observed: Scope,
    operation: RecordReadOperation,
    steps: usize,
) -> Result<RecordReadValue, RecordReadError> {
    let mut source = runtime
        .block_on(storage.record_source_expected(&session, &identity))
        .map_err(storage_error)?;
    exact_record_scope(&admitted, &source, &observed).map_err(RecordReadError::Admission)?;
    let value = match operation {
        RecordReadOperation::Head => {
            discover(steps, || source.bounded_head(&observed)).map(|head| {
                RecordReadValue::Head(RecordHead {
                    scope: observed,
                    head,
                })
            })
        }
        RecordReadOperation::Page(page) => {
            discover(steps, || source.bounded_page(&page)).map(RecordReadValue::Page)
        }
    };
    // The final SDK source drop joins its internal worker
    // here because this thread is outside a Tokio enter.
    drop(source);
    value
}

/// Ask the SDK again while it is still validating, up to `steps` calls. The
/// SDK keeps every step's progress, so a later read resumes where this one
/// stopped; the first refusal ends the read.
fn discover<T>(
    steps: usize,
    mut call: impl FnMut() -> Result<RecordReadStatus<T>, SourceError>,
) -> Result<T, RecordReadError> {
    for _ in 0..steps {
        if let RecordReadStatus::Ready(value) = call().map_err(source_error)? {
            return Ok(value);
        }
    }
    Err(RecordReadError::SourcePreparing)
}

pub(super) fn storage_error(error: StorageError) -> RecordReadError {
    match error {
        StorageError::IdentityMismatch => RecordReadError::IdentityChanged,
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
