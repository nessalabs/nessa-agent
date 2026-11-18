//! Immutable evidence of the exact pinned grantor's authority decision.
//! Observation clocks and durable serialization belong to the record adapter.
use nessa_auth::domain::{
    pairing::{DeviceKey, PublicIntent},
    CredentialId,
};

/// A current local decision became noncurrent because its pinned grantor
/// returned NotCurrent for this exact enrollment. The grantor is the system
/// initiator; no human revocation actor or remote effect time is inferred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NotCurrentEvidence {
    pub(crate) grantor: DeviceKey,
    pub(crate) intent: PublicIntent,
    pub(crate) credential: CredentialId,
}
impl NotCurrentEvidence {
    pub(crate) fn matching_grantor(
        grantor: DeviceKey,
        intent: PublicIntent,
        credential: CredentialId,
    ) -> Self {
        Self {
            grantor,
            intent,
            credential,
        }
    }

    /// A revoked phase no longer holds an active credential, but the original
    /// observation still belongs to the same enrollment and retains its ID.
    pub(crate) fn belongs_to(
        &self,
        grantor: DeviceKey,
        intent: PublicIntent,
        active: Option<&CredentialId>,
        revoked: bool,
    ) -> bool {
        self.grantor == grantor
            && self.intent == intent
            && match active {
                Some(credential) => credential == &self.credential && !revoked,
                None => revoked,
            }
    }
}
