use crate::domain::{
    pairing::{AttemptId, PairingError},
    CredentialId, ResourceId,
};
/// Exact physical result returned by the trusted canonical receiver authority.
/// Parsing fields conveys no device authority; the pairing owner correlates the durable stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverOutcome {
    credential: CredentialId,
    request: AttemptId,
    receiver: ResourceId,
    epoch: u64,
    generation: u64,
}
impl ReceiverOutcome {
    /// Validate bounded identifiers and positive canonical epoch/generation.
    pub fn new(
        credential: CredentialId,
        request: AttemptId,
        receiver: ResourceId,
        epoch: u64,
        generation: u64,
    ) -> Result<Self, PairingError> {
        if epoch == 0 || generation == 0 {
            return Err(PairingError::Invalid);
        }
        Ok(Self {
            credential,
            request,
            receiver,
            epoch,
            generation,
        })
    }
    /// Reserved credential this result physically targeted.
    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }
    /// Stable durable dispatch correlation.
    pub fn request(&self) -> AttemptId {
        self.request
    }
    /// Canonical receiver returned by the authority.
    pub fn receiver(&self) -> &ResourceId {
        &self.receiver
    }
    /// Canonical current result epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// Immutable enrollment generation covered by dispatch.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
