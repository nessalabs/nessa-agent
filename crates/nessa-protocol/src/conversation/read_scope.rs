//! The read scope a passive read is admitted for, and the rules that check a
//! source scope against it. The gateway checks what it is asked against these;
//! a device re-checks what the gateway answered against the same functions.
use super::domain::{conversation_catalogue_schema, conversation_catalogue_stream, ConversationId};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sync::replication::catalogue::CatalogueSourceError;
use nessa_sync::replication::domain::{Id, Scope};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRefusal {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    WrongOwner,
    WrongReceiver,
    StaleEpoch,
    Unverifiable,
}

/// Exact receiver and ownership portion of a source scope. The physical source
/// contributes its own origin, stream, incarnation and schema after admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverReadScope {
    pub receiver_id: String,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub conversation_id: ConversationId,
    pub access_epoch: u64,
}

/// Owner-scoped catalogue selector, independent of any conversation ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogueReadScope {
    pub receiver_id: String,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub access_epoch: u64,
}

/// Encode the trusted passive receiver and numeric epoch into opaque sync IDs.
pub fn passive_read_selector(receiver: &str, epoch: u64) -> Result<(Id, Id), ReadRefusal> {
    let receiver = Id::new(receiver).map_err(|_| ReadRefusal::Unverifiable)?;
    let epoch = Id::new(format!("epoch-{epoch}")).map_err(|_| ReadRefusal::Unverifiable)?;
    Ok((receiver, epoch))
}

/// Refuse a scope that names another receiver or another access epoch.
pub fn validate_passive_read_selector(
    receiver: &str,
    epoch: u64,
    scope: &Scope,
) -> Result<(), ReadRefusal> {
    let (receiver, epoch) = passive_read_selector(receiver, epoch)?;
    if scope.receiver() != &receiver {
        return Err(ReadRefusal::WrongReceiver);
    }
    if scope.access_epoch() != &epoch {
        return Err(ReadRefusal::StaleEpoch);
    }
    Ok(())
}

/// Refuse a record scope outside the admitted conversation, receiver or epoch.
pub fn validate_record_selector(
    admitted: &ReceiverReadScope,
    scope: &Scope,
) -> Result<(), ReadRefusal> {
    if scope.stream().as_str() != admitted.conversation_id.to_string() {
        return Err(ReadRefusal::WrongOwner);
    }
    validate_passive_read_selector(&admitted.receiver_id, admitted.access_epoch, scope)
}

/// Refuse a catalogue scope outside the admitted owner, receiver or epoch.
pub fn validate_catalogue_selector(
    admitted: &CatalogueReadScope,
    scope: &Scope,
) -> Result<(), ReadRefusal> {
    if scope.stream()
        != &conversation_catalogue_stream(&admitted.organization_id, &admitted.owner_id)
    {
        return Err(ReadRefusal::WrongOwner);
    }
    validate_passive_read_selector(&admitted.receiver_id, admitted.access_epoch, scope)
}

/// Check that `scope` names the conversation catalogue schema and this
/// owner's stream: the construction relationship, without I/O. The gateway's
/// catalogue source and a device reading what the gateway answered both ask
/// this one function.
pub fn check_catalogue_scope_identity(
    organization_id: &OrganizationId,
    principal_id: &PrincipalId,
    scope: &Scope,
) -> Result<(), CatalogueSourceError> {
    if scope.schema() != &conversation_catalogue_schema()
        || scope.stream() != &conversation_catalogue_stream(organization_id, principal_id)
    {
        return Err(CatalogueSourceError::IdentityChanged);
    }
    Ok(())
}
