//! SDK identity and physical reads on tracked threads after passive admission.

use super::operation;
use crate::conversation::application::{
    ReadRefusal, ReceiverReadScope, RecordReadError, RecordReadFuture, RecordReadLease,
    RecordReadOperation, RecordReadResponse, RecordReadSource,
};
use crate::conversation::infrastructure::read_workers::{ReadWorkerError, ReadWorkers};
use crate::conversation::infrastructure::record_scope_from_identity;
use nessa_sdk::{
    domain::agent_execution::sessions::SessionId, infrastructure::session_storage::RecordStorage,
};
use nessa_sync::replication::domain::Id;
use std::sync::Arc;
#[cfg(test)]
use std::sync::{Condvar, Mutex};
use tokio::runtime::Handle;
#[cfg(test)]
use tokio::sync::Notify;

/// One physical SDK source per read; no Agent or writer lease is opened.
pub struct NessaRecordReadSource {
    storage: Arc<RecordStorage>,
    origin: Id,
    runtime: Handle,
    workers: Arc<ReadWorkers>,
    #[cfg(test)]
    before_identity: Option<Arc<TestReadGate>>,
}

#[cfg(test)]
struct TestReadGate {
    entered: Notify,
    panic_after_release: bool,
    open: Mutex<bool>,
    released: Condvar,
}

#[cfg(test)]
impl TestReadGate {
    fn wait(&self) {
        self.entered.notify_one();
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.released.wait(open).unwrap();
        }
        assert!(!self.panic_after_release, "injected read worker panic");
    }

    fn release(&self) {
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
            #[cfg(test)]
            before_identity: None,
        }
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
            #[cfg(test)]
            let before_identity = self.before_identity.clone();
            self.workers
                .run("nessa-record-read", move || {
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
                    let result = operation::execute(
                        storage, runtime, session, identity, admitted, observed, operation,
                    );
                    result.map(|value| RecordReadResponse { value, lease })
                })
                .await
                .map_err(worker_error)?
        })
    }
}

fn worker_error(error: ReadWorkerError) -> RecordReadError {
    match error {
        ReadWorkerError::Unavailable => RecordReadError::TemporarilyUnavailable,
        ReadWorkerError::WorkerPanicked => RecordReadError::WorkerPanicked,
    }
}

fn session_id(admitted: &ReceiverReadScope) -> Result<SessionId, RecordReadError> {
    Ok(crate::conversation::application::conversation_session(
        &admitted.conversation_id,
    ))
}

#[cfg(test)]
#[path = "../../../../tests/conversation/record_read/source.rs"]
mod tests;
