//! The receiver a device enrollment pairs, as activation and cleanup ask for it.
//!
//! The canonical receiver authority owns receiver identity, epochs and their
//! journal; this port is how the enrollment use cases reach it. Every call is
//! correlated by the enrollment's durable stage, so a retry finds the original
//! receipt instead of pairing again.
use nessa_auth::{
    application::pairing::ReceiverOutcome,
    domain::{
        pairing::{AttemptId, PairingRecord},
        CredentialId, OrganizationId, PrincipalId, ResourceId,
    },
};

/// What a staged enrollment asks the receiver authority for: the reserved
/// credential, its stage correlation and generation, and the owner and
/// organization its consent names. Only a staged record yields one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverRequest {
    credential: CredentialId,
    request: AttemptId,
    generation: u64,
    organization: OrganizationId,
    owner: PrincipalId,
}
impl ReceiverRequest {
    /// The request for this record's durable stage; `None` before Stage.
    pub fn for_stage(record: &PairingRecord) -> Option<Self> {
        let (credential, request) = record.stage_binding()?;
        let intent = record.intent();
        Some(Self {
            credential: credential.clone(),
            request,
            generation: intent.generation(),
            organization: intent.resource().organization_id().clone(),
            owner: intent.owner().clone(),
        })
    }
    /// The credential identity the stage reserved.
    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }
    /// The stage's durable correlation; receipts are found by it.
    pub fn request(&self) -> AttemptId {
        self.request
    }
    /// The immutable consent generation the stage covers.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// The organization the receiver is bound in.
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    /// The owner the receiver reads for, and the principal who paired it.
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
}

/// A credential's current receiver binding, read when it is needed and never
/// kept: an epoch is stale the moment it is written down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentReceiver {
    /// The receiver identity.
    pub receiver: ResourceId,
    /// Its current access epoch.
    pub epoch: u64,
    /// Whether the binding currently admits reads.
    pub active: bool,
    /// The organization the receiver is bound in.
    pub organization: OrganizationId,
    /// The owner it reads for.
    pub owner: PrincipalId,
}

/// Why the receiver authority did not answer, without its diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverError {
    /// Storage was unavailable; nothing is known to have changed.
    Unavailable,
    /// The request contradicts an existing receipt or binding.
    Conflict,
    /// The named receiver does not exist.
    Missing,
    /// The receiver's epoch cannot advance further.
    Exhausted,
}

/// The canonical receiver authority, as device enrollment reaches it. Calls run
/// on the caller's thread: enrollment work is already on a blocking worker.
pub trait PairingReceivers: Send + Sync {
    /// Pair a receiver for this stage. A repeat of the same stage returns the
    /// original receipt, with no second receiver or epoch.
    fn pair(&self, request: &ReceiverRequest) -> Result<ReceiverOutcome, ReceiverError>;
    /// The original pair receipt for this stage, without pairing: `None` is a
    /// coherent absence, never an unavailable read.
    fn paired(&self, request: &ReceiverRequest) -> Result<Option<ReceiverOutcome>, ReceiverError>;
    /// The credential's current binding, if it has one.
    fn current(&self, credential: &CredentialId) -> Result<Option<CurrentReceiver>, ReceiverError>;
    /// Fence the receiver this stage paired at `paired_epoch`, as the system.
    /// A repeat returns the original fence; a receiver already revoked from
    /// this credential returns that revocation.
    fn fence(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<ReceiverOutcome, ReceiverError>;
}
