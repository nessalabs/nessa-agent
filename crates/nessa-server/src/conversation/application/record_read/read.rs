//! Fresh passive admission followed by one exact, bounded physical record read.

use super::super::{
    passive_read_selector, validate_passive_read_selector, AdmitPassiveRead, ReadRefusal,
    ReceiverReadScope,
};
use crate::conversation::domain::ConversationId;
use nessa_auth::application::session::AuthenticatedSession;
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

/// A single global capacity token that survives caller cancellation once handed
/// to the source thread. Its type is deliberately opaque to application logic.
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
/// The product writer retains it through physical delivery or socket teardown.
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

pub(crate) fn validate_record_selector(
    admitted: &ReceiverReadScope,
    scope: &Scope,
) -> Result<(), ReadRefusal> {
    if scope.stream().as_str() != admitted.conversation_id.to_string() {
        return Err(ReadRefusal::WrongOwner);
    }
    validate_passive_read_selector(&admitted.receiver_id, admitted.access_epoch, scope)
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
        let admitted = self
            .admission
            .execute(session, conversation, receiver_id, access_epoch)
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
