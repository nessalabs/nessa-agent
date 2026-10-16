//! Historical local enrollment adoption; current authority stays with fresh status.
use super::PrivateStateError;
use crate::domain::{
    pairing::{DisclosedConsent, OwnerPhoneRequest, PublicIntent},
    CredentialId, ResourceId,
};

/// The kind physically acknowledged by the explicit local adoption owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerPhoneEnrollmentKind {
    /// Exact public enrollment retained without issued selectors.
    Pending,
    /// Exact issued credential and receiver retained with the original identity.
    Issued,
}

/// Immutable local acknowledgement, not live read/write access or readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerPhoneAdoption {
    public: PublicIntent,
    kind: OwnerPhoneEnrollmentKind,
}
impl OwnerPhoneAdoption {
    /// Publish facts from the local storage owner; consumers correlate its result.
    pub fn new(public: PublicIntent, kind: OwnerPhoneEnrollmentKind) -> Self {
        Self { public, kind }
    }
    /// Exact acknowledged invitation/attempt/consent/generation/expiry/class.
    pub fn public(&self) -> PublicIntent {
        self.public
    }
    /// The historical stored record kind.
    pub fn kind(&self) -> OwnerPhoneEnrollmentKind {
        self.kind
    }
}

/// Where the original local failure occurred, without diagnostic inference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerPhoneAdoptionPhase {
    /// Restoring or correlating the actual original/enrollment refused.
    OriginalRead,
    /// The same original lineage could not be durably acknowledged.
    BindLineage,
    /// Original lineage was acknowledged; enrollment acknowledgement failed.
    PublishEnrollment,
}

/// Exact local failure, retaining a completed earlier acknowledgement separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerPhoneAdoptionFailure {
    /// No original lineage acknowledgement completed in this invocation.
    OriginalRead(PrivateStateError),
    /// No original lineage acknowledgement completed in this invocation.
    BindLineage(PrivateStateError),
    /// The exact original lineage acknowledgement completed before this failure.
    PublishEnrollment {
        /// Original finite publication/reconciliation failure, unchanged.
        error: PrivateStateError,
        /// The precise lineage acknowledged by this invocation's earlier owner.
        acknowledged_lineage: PublicIntent,
    },
}
impl OwnerPhoneAdoptionFailure {
    /// Original failure phase; earlier variants cannot construct an acknowledgement.
    pub fn phase(&self) -> OwnerPhoneAdoptionPhase {
        match self {
            Self::OriginalRead(_) => OwnerPhoneAdoptionPhase::OriginalRead,
            Self::BindLineage(_) => OwnerPhoneAdoptionPhase::BindLineage,
            Self::PublishEnrollment { .. } => OwnerPhoneAdoptionPhase::PublishEnrollment,
        }
    }
    /// Exact storage failure, including its physical effect and independent cleanup.
    pub fn error(&self) -> PrivateStateError {
        match self {
            Self::OriginalRead(error)
            | Self::BindLineage(error)
            | Self::PublishEnrollment { error, .. } => *error,
        }
    }
    /// The earlier acknowledgement when this invocation actually completed it.
    pub fn acknowledged_lineage(&self) -> Option<PublicIntent> {
        match self {
            Self::OriginalRead(_) | Self::BindLineage(_) => None,
            Self::PublishEnrollment {
                acknowledged_lineage,
                ..
            } => Some(*acknowledged_lineage),
        }
    }
}

/// Same-private-owner publication, called by the actual pinned recovery consumer.
pub trait OwnerPhoneEnrollmentStore: Send + Sync {
    /// Validate original seed/key/pin/digest/disclosure, durably bind its immutable
    /// lineage, then publish/reconcile its exact pending or issued enrollment.
    /// Input contains no incoming private seed, entropy or readiness flag.
    fn adopt_owner_phone(
        &self,
        original: &OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<OwnerPhoneAdoption, OwnerPhoneAdoptionFailure>;
}
