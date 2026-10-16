//! The receiver a device enrollment pairs, as activation and cleanup ask for it.
//!
//! The canonical receiver authority owns receiver identity, epochs and their
//! journal; this port is how the enrollment use cases reach it. Every call is
//! correlated by the enrollment's durable stage, so a retry finds the original
//! receipt instead of pairing again.
use nessa_auth::{
    application::pairing::{PairingStoreError, ReceiverOutcome, StageOwnership},
    domain::{
        pairing::{AttemptId, PairingError, PairingPhase, PairingRecord},
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

/// One mutable receiver dispatch borrowing actual noncloneable stage ownership.
/// Read-only selectors or a caller-constructed record cannot construct this lease.
pub struct ReceiverDispatch<'a> {
    _ownership: &'a StageOwnership,
    request: ReceiverRequest,
    kind: DispatchKind,
}
#[derive(PartialEq, Eq)]
enum DispatchKind {
    Pair,
    Fence {
        receiver: ResourceId,
        paired_epoch: u64,
    },
}
impl<'a> ReceiverDispatch<'a> {
    /// Admit an original Pair under its current live physical stage owner.
    /// Once admitted, a late physical outcome remains owned even if the record ends.
    pub fn for_pair(ownership: &'a StageOwnership) -> Result<Self, PairingStoreError> {
        let record = ownership.current()?;
        if record.phase() != PairingPhase::Staging || record.receiver_binding().is_some() {
            return Err(PairingStoreError::Domain(PairingError::Ineligible));
        }
        Self::new(ownership, &record, DispatchKind::Pair)
    }
    /// Admit fencing only for an ended original stage with its real receipt retained.
    pub fn for_fence(ownership: &'a StageOwnership) -> Result<Self, PairingStoreError> {
        let record = ownership.current()?;
        // PairingRecord::cleanup_pending owns the Terminal/unsettled-stage relation.
        if !record.cleanup_pending() {
            return Err(PairingStoreError::Domain(PairingError::Ineligible));
        }
        let (receiver, paired_epoch) = record
            .receiver_binding()
            .ok_or(PairingStoreError::Domain(PairingError::Ineligible))?;
        Self::new(
            ownership,
            &record,
            DispatchKind::Fence {
                receiver: receiver.clone(),
                paired_epoch,
            },
        )
    }
    fn new(
        ownership: &'a StageOwnership,
        record: &PairingRecord,
        kind: DispatchKind,
    ) -> Result<Self, PairingStoreError> {
        let request = ReceiverRequest::for_stage(record)
            .ok_or(PairingStoreError::Domain(PairingError::Conflict))?;
        Ok(Self {
            _ownership: ownership,
            request,
            kind,
        })
    }
    pub(crate) fn pair_request(&self) -> Result<&ReceiverRequest, ReceiverError> {
        if self.kind != DispatchKind::Pair {
            return Err(ReceiverError::Conflict);
        }
        Ok(&self.request)
    }
    pub(crate) fn fence_request(
        &self,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<&ReceiverRequest, ReceiverError> {
        if !matches!(&self.kind, DispatchKind::Fence { receiver: original, paired_epoch: epoch } if original == receiver && *epoch == paired_epoch)
        {
            return Err(ReceiverError::Conflict);
        }
        Ok(&self.request)
    }
}
impl std::ops::Deref for ReceiverDispatch<'_> {
    type Target = ReceiverRequest;
    fn deref(&self) -> &Self::Target {
        &self.request
    }
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
    /// The receiver no longer holds the stage's pairing: inactive, another
    /// binding, or its credential revoked on it since the pair.
    NotPaired,
}

/// Held across a receiver write and the credential publish that follows it.
/// Dropping it releases the lock. A test double may hold no file.
pub struct PublicationGuard {
    _file: Option<std::fs::File>,
}

impl PublicationGuard {
    pub fn held(file: std::fs::File) -> Self {
        Self { _file: Some(file) }
    }

    /// No file. Test doubles that do not share a namespace use this.
    pub fn none() -> Self {
        Self { _file: None }
    }
}

/// The canonical receiver authority, as device enrollment reaches it. Calls run
/// on the caller's thread: enrollment work is already on a blocking worker.
pub trait PairingReceivers: Send + Sync {
    /// Exclusive lock held from a receiver write through the credential
    /// publish. Export takes the same file while it snapshots both stores.
    fn lock_publication(&self) -> Result<PublicationGuard, ReceiverError>;
    /// Pair a receiver for this stage. A repeat of the same stage returns the
    /// original receipt, with no second receiver or epoch.
    fn pair(&self, dispatch: &ReceiverDispatch<'_>) -> Result<ReceiverOutcome, ReceiverError>;
    /// The original pair receipt for this stage, without pairing: `None` is a
    /// coherent absence, never an unavailable read.
    fn paired(&self, request: &ReceiverRequest) -> Result<Option<ReceiverOutcome>, ReceiverError>;
    /// The receiver's current epoch if it still holds this stage's pairing at
    /// `paired_epoch` (active, the same receiver, credential, organization and
    /// owner, and the credential not revoked on it since); `None` otherwise.
    fn holding(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<Option<u64>, ReceiverError>;
    /// Fence the receiver this stage paired at `paired_epoch`, as the system.
    /// A repeat returns the original fence; a receiver already revoked from
    /// this credential returns that revocation.
    fn fence(
        &self,
        dispatch: &ReceiverDispatch<'_>,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<ReceiverOutcome, ReceiverError>;
}
