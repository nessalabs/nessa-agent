//! Explicit principal selection for canonical records, distinct from a receiver.
use super::{RecordReadFuture, RecordReadLease, RecordReadOperation, RecordReadResponse};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::{domain::ConversationId, read_scope::ReadRefusal};
use nessa_sync::replication::domain::{Id, Scope};
use uuid::Uuid;

/// A checked selector, not an authorization decision or a receiver binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrincipalRecordScope {
    registration: Uuid,
    principal: PrincipalId,
    organization: OrganizationId,
    owner: PrincipalId,
    conversation: ConversationId,
    epoch: u64,
}
impl PrincipalRecordScope {
    pub(crate) fn new(
        registration: Uuid,
        principal: PrincipalId,
        organization: OrganizationId,
        owner: PrincipalId,
        conversation: ConversationId,
        epoch: u64,
    ) -> Self {
        Self {
            registration,
            principal,
            organization,
            owner,
            conversation,
            epoch,
        }
    }
    /// Actual granted principal selected by application admission.
    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }
    /// Organization of the admitted conversation and principal.
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    /// Conversation owner checked by the existing Cedar policy.
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    /// Exact conversation whose physical records are selected.
    pub fn conversation(&self) -> &ConversationId {
        &self.conversation
    }
    pub(crate) fn selector(&self) -> (Id, Id) {
        (
            Id::new(self.registration.to_string()).expect("bounded UUID selector"),
            Id::new(format!("epoch-{}", self.epoch)).expect("bounded numeric epoch"),
        )
    }
    pub(crate) fn validate(&self, scope: &Scope) -> Result<(), ReadRefusal> {
        let (subject, epoch) = self.selector();
        if scope.receiver() != &subject {
            return Err(ReadRefusal::WrongReceiver);
        }
        if scope.access_epoch() != &epoch {
            return Err(ReadRefusal::StaleEpoch);
        }
        if scope.stream().as_str() != self.conversation.to_string() {
            return Err(ReadRefusal::WrongOwner);
        }
        Ok(())
    }
}

/// Physical I/O behind current principal authority. Implementations retain the
/// supplied lease through worker/source join even when the waiter disappears.
pub trait PrincipalRecordReadSource: Send + Sync {
    /// Read one head or page without creating a paired receiver or an Agent.
    /// The caller must acquire current principal authority before invoking I/O.
    fn read_principal<'a>(
        &'a self,
        admitted: PrincipalRecordScope,
        operation: RecordReadOperation,
        lease: RecordReadLease,
    ) -> RecordReadFuture<'a, RecordReadResponse>;
}
