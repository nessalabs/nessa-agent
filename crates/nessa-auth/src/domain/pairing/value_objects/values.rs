use super::wire_values::{DEVICE_KEY_BYTES, IDENTITY_BYTES};
use crate::domain::{
    models::{CONVERSATION_READ, ENVIRONMENT_SERVE},
    Action, AudienceId, Grant, Initiator, MembershipId, OrganizationId, PrincipalId, Resource,
};
use std::{error::Error, fmt};

const MAX_ATTEMPTS: u8 = 5;
const DEVICE_READ_CLASS: &str = "gateway-conversation-read";
const PEER_READ_CLASS: &str = "peer-gateway-conversation-read";
const PEER_ENVIRONMENT_CLASS: &str = "peer-gateway-environment-serve";
const OWNER_PHONE_CLASS: &str = "gateway-owner-conversation-read-write";
/// Prefix of the principal id a peer gateway's credential names. The
/// registry reserves it: a principal's id has it exactly when its kind is a
/// gateway.
pub const PEER_PRINCIPAL_PREFIX: &str = "gateway:";

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

/// Who an enrollment enrolls, and the capabilities the credential it issues
/// holds.
///
/// Every class uses the same enrollment and listener. A device's credential
/// names the owner and reads; a peer's names the gateway principal of the key
/// it pinned and either reads what it is granted or serves as an environment,
/// never both under one credential. The class is public, bound into the PAKE
/// transcript, and fixed when the owner creates the invitation, so an
/// enrolling party cannot become a different kind of principal, or hold a
/// different grant, than the owner invited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConsentClass {
    /// One of the owner's own devices: the credential names the owner.
    DeviceRead,
    /// A peer gateway that reads: the credential names a principal of kind
    /// `PrincipalKind::Gateway`, identified by the key it pinned, which pairing
    /// creates (see [`peer_principal`]).
    PeerRead,
    /// A peer gateway or worker that serves as an environment: the same
    /// principal as [`Self::PeerRead`] for the same key, holding
    /// `environment.serve` and no read.
    PeerEnvironment,
    /// The owner's own phone with read and write over their conversations,
    /// admitted by a signed original request or by a code the owner then
    /// approves for the exact key.
    OwnerPhoneReadWrite,
}
impl ConsentClass {
    /// Every class, in no particular order.
    pub const ALL: [Self; 4] = [
        Self::DeviceRead,
        Self::PeerRead,
        Self::PeerEnvironment,
        Self::OwnerPhoneReadWrite,
    ];
    /// The public class name, as the wire and the PAKE transcript carry it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeviceRead => DEVICE_READ_CLASS,
            Self::PeerRead => PEER_READ_CLASS,
            Self::PeerEnvironment => PEER_ENVIRONMENT_CLASS,
            Self::OwnerPhoneReadWrite => OWNER_PHONE_CLASS,
        }
    }
    /// The class a public name denotes; `None` for any other text.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.as_str() == name)
    }
    /// Whether it enrolls a peer gateway, whose credential names the gateway
    /// principal of its key rather than the owner.
    pub fn is_peer(self) -> bool {
        match self {
            Self::DeviceRead | Self::OwnerPhoneReadWrite => false,
            Self::PeerRead | Self::PeerEnvironment => true,
        }
    }
    /// The principal a credential issued to `key` under this class names:
    /// the owner for a device, and for a peer the gateway principal of that
    /// key. This is the one place that decides it.
    pub fn credential_principal(
        self,
        owner: &PrincipalId,
        key: &DeviceKey,
    ) -> Result<PrincipalId, PairingError> {
        match self.is_peer() {
            false => Ok(owner.clone()),
            true => peer_principal(key),
        }
    }
    /// The primary action of a credential issued under this class
    /// holds. This is the one place that decides it.
    pub fn grant_action(self) -> &'static str {
        match self {
            Self::DeviceRead | Self::PeerRead | Self::OwnerPhoneReadWrite => CONVERSATION_READ,
            Self::PeerEnvironment => ENVIRONMENT_SERVE,
        }
    }
    /// Whether that grant is one the owner hands on from their own, so the
    /// owner creating the invitation must hold it too. A read is; serving is
    /// not: no person serves as an environment, and admitting a peer as one
    /// is managing this gateway's credentials, which `credential.manage`
    /// decides alone.
    pub fn delegates_owner_grant(self) -> bool {
        match self {
            Self::DeviceRead | Self::PeerRead | Self::OwnerPhoneReadWrite => true,
            Self::PeerEnvironment => false,
        }
    }
    /// That grant, on the gateway `resource`.
    pub fn grant(self, resource: Resource) -> Result<Grant, PairingError> {
        Ok(Grant::new(
            Action::new(self.grant_action()).map_err(|_| PairingError::Invalid)?,
            resource,
        ))
    }
    /// Whether a client that enrolls as this class takes an invitation of
    /// `invited`. A device client takes either kind of the owner's own
    /// device: read only, or the phone's read and write. The owner chose which
    /// when they made the invitation. A peer takes only its own class.
    pub fn accepts(self, invited: Self) -> bool {
        self == invited || (self == Self::DeviceRead && invited == Self::OwnerPhoneReadWrite)
    }
    /// Complete canonical ordered capabilities. Owner-phone method/resource
    /// admission remains separate from these coarse Cedar actions.
    pub fn grants(self, resource: Resource) -> Result<Vec<Grant>, PairingError> {
        let mut grants = vec![self.grant(resource.clone())?];
        if self == Self::OwnerPhoneReadWrite {
            for action in ["conversation.write", "server.read"] {
                grants.push(Grant::new(
                    Action::new(action).map_err(|_| PairingError::Invalid)?,
                    resource.clone(),
                ));
            }
        }
        Ok(grants)
    }
}

/// The id of the gateway principal a peer key enrolls as: the fixed prefix and
/// the key in lowercase hex. One key is one peer, so pairing the same peer
/// again names the same principal.
pub fn peer_principal(key: &DeviceKey) -> Result<PrincipalId, PairingError> {
    let mut id = String::with_capacity(PEER_PRINCIPAL_PREFIX.len() + 2 * DEVICE_KEY_BYTES);
    id.push_str(PEER_PRINCIPAL_PREFIX);
    for byte in key.bytes() {
        id.push(char::from_digit(u32::from(byte >> 4), 16).ok_or(PairingError::Invalid)?);
        id.push(char::from_digit(u32::from(byte & 0x0f), 16).ok_or(PairingError::Invalid)?);
    }
    PrincipalId::new(id).map_err(|_| PairingError::Invalid)
}

/// Whether `principal` is a paired peer gateway's. The registry holds an id
/// with [`PEER_PRINCIPAL_PREFIX`] exactly when its principal's kind is a
/// gateway, so this answers the kind for anyone holding only the id.
pub fn is_peer_principal(principal: &PrincipalId) -> bool {
    principal.as_str().starts_with(PEER_PRINCIPAL_PREFIX)
}

/// One immutable gateway consent, with owner linkage private.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentIntent {
    id: ConsentIntentId,
    generation: u64,
    audience: AudienceId,
    owner: PrincipalId,
    membership: MembershipId,
    grant: Grant,
    additional_grants: Vec<Grant>,
    class: ConsentClass,
}
impl ConsentIntent {
    /// Who this consent enrolls; fixed when the owner created it.
    pub fn class(&self) -> ConsentClass {
        self.class
    }

    /// Build a consent of `class` for an authoritative gateway resource,
    /// asking for that class's complete canonical capabilities.
    /// Generation zero is invalid; policy evaluation remains with Cedar.
    pub fn new(
        id: ConsentIntentId,
        generation: u64,
        audience: AudienceId,
        owner: PrincipalId,
        membership: MembershipId,
        resource: Resource,
        class: ConsentClass,
    ) -> Result<Self, PairingError> {
        if generation == 0 {
            return Err(PairingError::Invalid);
        }
        let mut grants = class.grants(resource)?;
        let grant = grants.remove(0);
        Ok(Self {
            id,
            generation,
            audience,
            owner,
            membership,
            grant,
            additional_grants: grants,
            class,
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
    /// Exact gateway resource the consent's grant is on.
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
    /// The primary canonical grant; authority must evaluate the complete `grants` set.
    pub fn grant(&self) -> &Grant {
        &self.grant
    }
    /// Full immutable capability set evaluated and issued together.
    pub fn grants(&self) -> impl Iterator<Item = &Grant> {
        std::iter::once(&self.grant).chain(self.additional_grants.iter())
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
