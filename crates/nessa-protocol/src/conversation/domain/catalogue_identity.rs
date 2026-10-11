//! Stable catalogue identity for an authenticated metadata owner: the schema
//! every conversation catalogue scope names, and the stream one owner's
//! catalogue is read from.

use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sync::replication::domain::Id;
use sha2::{Digest, Sha256};

const SCHEMA: &str = "nessa.conversation-catalogue.v1";
/// What every owner's stream starts with, before its digest.
const STREAM_PREFIX: &str = "conversation-owner:";

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
    Id::new(format!("{STREAM_PREFIX}{:x}", hash.finalize())).expect("digest fits sync ID")
}

/// Whether `stream` has the shape of some owner's catalogue stream, for a
/// reader that is not the owner and cannot name it: a peer gateway reading
/// what another gateway's owner granted it.
pub fn is_conversation_catalogue_stream(stream: &Id) -> bool {
    stream
        .as_str()
        .strip_prefix(STREAM_PREFIX)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_identity.rs"]
mod tests;
