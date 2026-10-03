use super::wire_values::{DEVICE_KEY_BYTES, IDENTITY_BYTES};
use crate::domain::{
    Action, AudienceId, Grant, Initiator, MembershipId, OrganizationId, PrincipalId, Resource,
};
use std::{error::Error, fmt};

const MAX_ATTEMPTS: u8 = 5;
pub(super) const READ_CLASS: &str = "gateway-conversation-read";

/// Expected pairing refusal; operational adapter failures are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingError {
    /// Policy or event contains an invalid relationship.
    Invalid,
    /// Invitation expiry is exclusive and has been reached.
    Expired,
    /// No new attempt can be admitted.
    AttemptsExhausted,
    /// A pending worker or configured resource bound is occupied.
    Capacity,
    /// This gateway already owns an Available manual-code invitation.
    AvailableSlotOccupied,
    /// Event conflicts with an earlier owned identity, target or outcome.
    Conflict,
    /// This phase cannot accept the event.
    Ineligible,
    /// Actor does not own this consent or attempt.
    WrongActor,
    /// Generation changed after earlier admission.
    StaleGeneration,
}
impl fmt::Display for PairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for PairingError {}

macro_rules! identity {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name([u8; IDENTITY_BYTES]);
        impl $name {
            /// Published fixed identity width, derived from owner contract data.
            pub const LENGTH: usize = IDENTITY_BYTES;
            /// Preserve a server/client allocated random 128-bit identity.
            pub fn new(bytes: [u8; IDENTITY_BYTES]) -> Self {
                Self(bytes)
            }
            /// Borrow the exact identity bytes.
            pub fn bytes(&self) -> &[u8; IDENTITY_BYTES] {
                &self.0
            }
        }
    };
}
identity!(
    InvitationId,
    "Public random locator for a one-use invitation."
);
identity!(
    AttemptId,
    "Immutable admission identity for one charged PAKE attempt."
);
identity!(
    ConsentIntentId,
    "Opaque random identity of one immutable full consent intent."
);

/// Fixed Ed25519 public key bytes; construction is not a proof of possession.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceKey([u8; DEVICE_KEY_BYTES]);
impl DeviceKey {
    /// Published fixed key width, derived from owner contract data.
    pub const LENGTH: usize = DEVICE_KEY_BYTES;
    /// Preserve public key bytes already decoded by the canonical boundary codec.
    pub fn new(bytes: [u8; DEVICE_KEY_BYTES]) -> Self {
        Self(bytes)
    }
    /// Borrow key bytes for exact binding comparisons.
    pub fn bytes(&self) -> &[u8; DEVICE_KEY_BYTES] {
        &self.0
    }
}

/// One immutable gateway conversation-read consent, with owner linkage private.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentIntent {
    id: ConsentIntentId,
    generation: u64,
    audience: AudienceId,
    owner: PrincipalId,
    membership: MembershipId,
    grant: Grant,
}
impl ConsentIntent {
    /// Publish the sole gateway-read grant representation for boundary projections.
    pub fn read_grant(resource: Resource) -> Result<Grant, PairingError> {
        let action = Action::new("conversation.read").map_err(|_| PairingError::Invalid)?;
        Ok(Grant::new(action, resource))
    }

    /// Fixed public class owned by this immutable gateway-read consent.
    pub fn class(&self) -> &'static str {
        READ_CLASS
    }

    /// Build the sole read class for an authoritative gateway resource.
    /// Generation zero is invalid; policy evaluation remains with Cedar.
    pub fn new(
        id: ConsentIntentId,
        generation: u64,
        audience: AudienceId,
        owner: PrincipalId,
        membership: MembershipId,
        resource: Resource,
    ) -> Result<Self, PairingError> {
        if generation == 0 {
            return Err(PairingError::Invalid);
        }
        let grant = Self::read_grant(resource)?;
        Ok(Self {
            id,
            generation,
            audience,
            owner,
            membership,
            grant,
        })
    }
    /// Public opaque consent identity.
    pub fn id(&self) -> ConsentIntentId {
        self.id
    }
    /// Desired generation this intent belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Deployment resolved by its auth owner.
    pub fn audience(&self) -> &AudienceId {
        &self.audience
    }
    /// Principal whose exact key enrollment requires explicit consent.
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    /// Existing membership; enrollment does not replace it.
    pub fn membership(&self) -> &MembershipId {
        &self.membership
    }
    /// Exact gateway resource whose conversation reads may be delegated.
    pub fn resource(&self) -> &Resource {
        self.grant.resource()
    }
    /// Match immutable owner/resource selectors for candidate discovery or admission.
    /// This relation grants no current access; session and policy owners still decide.
    pub fn matches_owner(
        &self,
        principal: &PrincipalId,
        membership: &MembershipId,
        audience: &AudienceId,
        organization: &OrganizationId,
        gateway: &Resource,
    ) -> bool {
        principal == self.owner()
            && membership == self.membership()
            && audience == self.audience()
            && organization == self.resource().organization_id()
            && gateway == self.resource()
            && gateway.id().as_str() == self.audience().as_str()
    }
    /// The canonical exact grant published by this read-only consent class.
    pub fn grant(&self) -> &Grant {
        &self.grant
    }
}

/// Finite invitation admission limits supplied by composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairingPolicy {
    lifetime_ms: u64,
    attempts: u8,
}
impl PairingPolicy {
    /// Validate an exclusive positive lifetime and one to five attempts.
    pub fn new(lifetime_ms: u64, attempts: u8) -> Result<Self, PairingError> {
        if lifetime_ms == 0 || attempts == 0 || attempts > MAX_ATTEMPTS {
            return Err(PairingError::Invalid);
        }
        Ok(Self {
            lifetime_ms,
            attempts,
        })
    }
    /// Accepted initial composition policy: ten minutes and five attempts.
    pub fn initial() -> Self {
        Self {
            lifetime_ms: 600_000,
            attempts: MAX_ATTEMPTS,
        }
    }
    /// Positive lifetime in milliseconds.
    pub fn lifetime_ms(&self) -> u64 {
        self.lifetime_ms
    }
    /// Maximum durably charged attempts per invitation.
    pub fn attempts(&self) -> u8 {
        self.attempts
    }
}

/// Known actor of a pairing decision; a device key is not a human attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingInitiator {
    /// Actual operating-system initiator of a canonical credential recovery/revocation.
    LocalOperator,
    /// Current authenticated owner principal.
    Principal(PrincipalId),
    /// Device whose TLS possession is verified by the application boundary.
    Device(DeviceKey),
    /// Gateway deadline, restart, or physical receiver reconciliation.
    System,
}

impl PairingInitiator {
    /// Preserve the actual initiator of the canonical credential transition.
    pub fn from_credential(initiator: &Initiator) -> Self {
        match initiator {
            Initiator::Principal(id) => Self::Principal(id.clone()),
            Initiator::LocalOperator => Self::LocalOperator,
        }
    }
}
