//! Fresh passive admission followed by one exact, bounded physical record read.

use super::super::{AdmitPassiveRead, PassiveRead};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::{
    passive_read_selector, validate_record_selector, ReadRefusal, ReceiverReadScope,
};
use nessa_sync::replication::domain::{Page, PageRequest, Scope};
use std::{future::Future, pin::Pin};

/// Refusals keep admission, physical identity, and source failures distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordReadError {
    Admission(ReadRefusal),
    InvalidRequest,
    IdentityChanged,
    HistoryPruned,
    RecordTooLarge,
    TemporarilyUnavailable,
    /// Unexpected outer worker panic, retained for process cleanup diagnostics.
    WorkerPanicked,
    ReadTimeout,
    SourcePreparing,
}

/// One fixed operation on a freshly opened physical source.
pub enum RecordReadOperation {
    Head,
    Page(PageRequest),
}

/// The bounded result before product encoding.
pub struct RecordHead {
    pub scope: Scope,
    pub head: u64,
}

pub enum RecordReadValue {
    Head(RecordHead),
    Page(Page),
}

/// Read capacity ownership that survives caller cancellation once handed to the
/// source thread. The product socket places its global permit and shared socket
/// slot in this opaque token; application logic does not inspect them.
pub struct RecordReadLease {
    _token: Box<dyn Send>,
}

impl RecordReadLease {
    pub fn new<T: Send + 'static>(token: T) -> Self {
        Self {
            _token: Box::new(token),
        }
    }
}

/// The read thread returns its token only after its SDK source is fully joined.
/// The product writer retains it until the response is encoded and checked
/// for sending (row R64), or until socket teardown.
pub struct RecordReadResponse {
    pub value: RecordReadValue,
    pub lease: RecordReadLease,
}

pub type RecordReadFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecordReadError>> + Send + 'a>>;

/// Source I/O is injected behind freshly admitted receiver authority.
pub trait RecordReadSource: Send + Sync {
    fn read<'a>(
        &'a self,
        admitted: ReceiverReadScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse>;
}

/// One call reauthorizes, checks physical identity, then reads a bounded result.
pub struct ReadRecords<'a> {
    pub admission: AdmitPassiveRead<'a>,
    pub source: &'a dyn RecordReadSource,
}

impl ReadRecords<'_> {
    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
        conversation: &ConversationId,
        receiver_id: &str,
        access_epoch: u64,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> Result<RecordReadResponse, RecordReadError> {
        let read = match &operation {
            RecordReadOperation::Head => PassiveRead::RecordHead,
            RecordReadOperation::Page(_) => PassiveRead::RecordPage,
        };
        let admitted = self
            .admission
            .execute(session, conversation, receiver_id, access_epoch, read)
            .await
            .map_err(RecordReadError::Admission)?;
        passive_read_selector(&admitted.receiver_id, admitted.access_epoch)
            .map_err(RecordReadError::Admission)?;
        let expected = match &operation {
            RecordReadOperation::Head => None,
            RecordReadOperation::Page(request) => {
                validate_record_selector(&admitted, &request.scope)
                    .map_err(RecordReadError::Admission)?;
                Some(request.clone())
            }
        };
        let response = self.source.read(admitted.clone(), operation, lease).await?;
        let scope = match (&expected, &response.value) {
            (None, RecordReadValue::Head(head)) => &head.scope,
            (Some(request), RecordReadValue::Page(page)) if &page.request == request => {
                &page.request.scope
            }
            _ => return Err(RecordReadError::Admission(ReadRefusal::Unverifiable)),
        };
        validate_record_selector(&admitted, scope).map_err(RecordReadError::Admission)?;
        Ok(response)
    }
}
