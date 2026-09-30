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

pub(super) fn execute(
    storage: Arc<RecordStorage>,
    runtime: Handle,
    session: SessionId,
    identity: RecordStreamIdentity,
    admitted: ReceiverReadScope,
    observed: Scope,
    operation: RecordReadOperation,
) -> Result<RecordReadValue, RecordReadError> {
    let mut source = runtime
        .block_on(storage.record_source_expected(&session, &identity))
        .map_err(storage_error)?;
    exact_record_scope(&admitted, &source, &observed).map_err(RecordReadError::Admission)?;
    let value = match operation {
        RecordReadOperation::Head => source
            .bounded_head(&observed)
            .map_err(source_error)
            .and_then(|status| match status {
                RecordReadStatus::Ready(head) => Ok(RecordReadValue::Head(RecordHead {
                    scope: observed,
                    head,
                })),
                RecordReadStatus::Preparing => Err(RecordReadError::SourcePreparing),
            }),
        RecordReadOperation::Page(page) => source
            .bounded_page(&page)
            .map_err(source_error)
            .and_then(|status| match status {
                RecordReadStatus::Ready(page) => Ok(RecordReadValue::Page(page)),
                RecordReadStatus::Preparing => Err(RecordReadError::SourcePreparing),
            }),
    };
    // The final SDK source drop joins its internal worker
    // here because this thread is outside a Tokio enter.
    drop(source);
    value
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
