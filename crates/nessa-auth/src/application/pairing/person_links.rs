//! Attribution follows the canonical enrollment and credential history. These
//! ports coordinate its grant-store effect; they do not decide access policy.
use crate::{
    application::revocation::CredentialRevocation,
    domain::{pairing::PairingRecord, Credential},
};

/// Coherent retained evidence supplied by the credential authority. Consumers
/// validate the record, credential and revocation together before effects.
#[derive(Clone)]
pub struct PairingLinkEvidence {
    /// The immutable invitation and its current canonical phase.
    pub record: PairingRecord,
    /// Current metadata for exactly that invitation's credential.
    pub credential: Credential,
    /// Original revocation, when present; the retry is not a new initiator.
    pub revocation: Option<CredentialRevocation>,
}

/// Operational or inconsistent attribution evidence; neither confers authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersonLinkError {
    /// The grant-store effect did not commit.
    Unavailable,
    /// The proposed evidence conflicts with an existing identity or lifecycle.
    Conflict,
}

/// The grant owner persists attribution and audit together, after credential
/// publication. Its methods must not read back through the credential authority:
/// the caller holds that authority's mutation mutex across these calls.
pub trait PairingPersonLinks: Send + Sync {
    /// Apply owed links and removals from one coherent registry cut.
    fn reconcile(&self, evidence: &[PairingLinkEvidence]) -> Result<(), PersonLinkError>;
    /// Refuse activation while any earlier link remains for the gateway key.
    fn require_absent(&self, record: &PairingRecord) -> Result<(), PersonLinkError>;
}

/// The next grant write uses the same reconciler under registry serialization.
/// The callback must finish before the source releases that serialization;
/// it must not call the source again or perform a credential mutation.
pub trait PairingLinkSource: Send + Sync {
    /// Run a grant-store effect with current retained enrollment evidence.
    fn with_current_links(
        &self,
        effect: &mut dyn FnMut(&[PairingLinkEvidence]) -> Result<(), PersonLinkError>,
    ) -> Result<(), PersonLinkError>;
}
