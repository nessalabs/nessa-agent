//! Stable catalogue identity for an authenticated metadata owner.

use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sync::replication::domain::Id;
use sha2::{Digest, Sha256};

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

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_identity.rs"]
mod tests;
