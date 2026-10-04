//! Stable catalogue identity for an authenticated metadata owner: the schema
//! every conversation catalogue scope names, and the stream one owner's
//! catalogue is read from.

use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sync::replication::catalogue::CatalogueSourceError;
use nessa_sync::replication::domain::{Id, Scope};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "nessa.conversation-catalogue.v1";

/// The schema every conversation catalogue scope names.
pub fn conversation_catalogue_schema() -> Id {
    Id::new(SCHEMA).expect("fixed schema ID")
}

/// Stable stream identity for one authenticated organization and principal.
/// The length prefix prevents two distinct owner pairs from hashing the same
/// byte sequence; the digest keeps the sync ID within its fixed size bound.
pub fn conversation_catalogue_stream(organization: &OrganizationId, principal: &PrincipalId) -> Id {
    let organization = organization.as_str().as_bytes();
    let owner = principal.as_str().as_bytes();
    let mut hash = Sha256::new();
    hash.update((organization.len() as u64).to_be_bytes());
    hash.update(organization);
    hash.update(owner);
    Id::new(format!("conversation-owner:{:x}", hash.finalize())).expect("digest fits sync ID")
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

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_identity.rs"]
mod tests;
