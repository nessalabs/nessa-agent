//! Current principal admission before bounded canonical physical record reads.
use super::{AdmissionFailure, AdmitDelivery, DeliveryHold, Registration};
use crate::conversation::application::{
    PrincipalRecordReadSource, PrincipalRecordScope, RecordReadError, RecordReadLease,
    RecordReadOperation, RecordReadResponse, RecordReadValue,
};
use nessa_auth::application::ports::Clock;
use nessa_protocol::conversation::{domain::ConversationId, read_scope::ReadRefusal};
use nessa_sync::replication::domain::{
    validate_page, validate_page_request, Checkpoint, Limits, Page,
};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebhookReadFailure {
    /// Existing credential, share-grant or Cedar admission refused/unavailable.
    Admission(AdmissionFailure),
    /// Canonical source selection, response or physical-worker failure.
    Source(RecordReadError),
}

/// A bounded source result, with the original credential/grant fences retained.
/// It is neither a stored cursor nor permission to POST. Inspection rechecks
/// absolute expiry, which no mutation fence can suspend.
pub struct WebhookRecords<'a> {
    response: RecordReadResponse,
    authority: Arc<Mutex<DeliveryHold>>,
    clock: &'a dyn Clock,
}
impl WebhookRecords<'_> {
    /// Borrow the bounded result while the original fences and expiry hold.
    pub fn value(&self) -> Result<&RecordReadValue, WebhookReadFailure> {
        let hold = self
            .authority
            .lock()
            .map_err(|_| WebhookReadFailure::Admission(AdmissionFailure::Unavailable))?;
        if !hold.is_valid_at(self.clock.unix_seconds()) {
            return Err(WebhookReadFailure::Admission(AdmissionFailure::Refused));
        }
        Ok(&self.response.value)
    }
}

/// Source-only use case; composition supplies committed private registration
/// facts and the same global capacity/page limits as other canonical readers.
pub struct ReadWebhookRecords<'a> {
    /// Existing Auth, principal-grant and Cedar owners.
    pub admission: AdmitDelivery<'a>,
    /// Canonical physical reader with tracked source cleanup.
    pub source: &'a dyn PrincipalRecordReadSource,
    /// The composition-owned global record budget, shared with other readers.
    pub capacity: &'a Arc<Semaphore>,
    /// Canonical page ceilings chosen by composition, never by source replies.
    pub limits: Limits,
}
impl<'a> ReadWebhookRecords<'a> {
    /// Read under current authority without queueing, advancing a cursor or
    /// sending. A later sender must independently re-admit its own attempt.
    pub async fn execute(
        &self,
        registration: &Registration,
        conversation: &ConversationId,
        epoch: u64,
        operation: RecordReadOperation,
    ) -> Result<WebhookRecords<'a>, WebhookReadFailure> {
        let permit = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| WebhookReadFailure::Source(RecordReadError::TemporarilyUnavailable))?;
        // Same ordering as send admission: credential, grant, metadata/Cedar.
        // No physical-worker lock is held while either authority is acquired.
        let hold = self
            .admission
            .execute(&registration.target, conversation, epoch)
            .await
            .map_err(WebhookReadFailure::Admission)?;
        let target = &registration.target;
        let selected = PrincipalRecordScope::new(
            registration.id,
            target.principal.clone(),
            target.organization.clone(),
            target.owner.clone(),
            conversation.clone(),
            epoch,
        );
        let expected = match &operation {
            RecordReadOperation::Head => None,
            RecordReadOperation::Page(request) => {
                selected.validate(&request.scope).map_err(refused)?;
                validate_page_request(request, self.limits)
                    .map_err(|_| WebhookReadFailure::Source(RecordReadError::InvalidRequest))?;
                Some(request.clone())
            }
        };
        // DeliveryHold is Send, not Sync. The mutex permits shared ownership;
        // no mutex guard crosses await or is acquired by the physical worker.
        // Cancellation drops our copy; the real worker's lease keeps the same
        // holds until its SDK source has joined. A normal result keeps both.
        let authority = Arc::new(Mutex::new(hold));
        let mut response = self
            .source
            .read_principal(
                selected.clone(),
                operation,
                RecordReadLease::new((permit, authority.clone())),
            )
            .await
            .map_err(WebhookReadFailure::Source)?;
        let scope = match (&expected, &response.value) {
            (None, RecordReadValue::Head(head)) => &head.scope,
            (Some(request), RecordReadValue::Page(page)) if &page.request == request => {
                &page.request.scope
            }
            _ => return Err(refused(ReadRefusal::Unverifiable)),
        };
        selected.validate(scope).map_err(refused)?;
        if let Some(request) = expected {
            let RecordReadValue::Page(page) = response.value else {
                unreachable!("checked page variant")
            };
            let plan = validate_page(
                &Checkpoint::new(request.scope.clone(), request.after),
                &request,
                page,
                self.limits,
            )
            .map_err(|_| refused(ReadRefusal::Unverifiable))?;
            let (_, _, records) = plan.into_parts();
            response.value = RecordReadValue::Page(Page { request, records });
        }
        let result = WebhookRecords {
            response,
            authority,
            clock: self.admission.clock,
        };
        result.value()?;
        Ok(result)
    }
}
fn refused(reason: ReadRefusal) -> WebhookReadFailure {
    WebhookReadFailure::Source(RecordReadError::Admission(reason))
}
