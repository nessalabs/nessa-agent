//! Original private reservation and seed publication, without remote authority.
use super::{PrivateKeyMaterial, PrivateStateError};
use crate::domain::pairing::{OwnerPhoneContext, OwnerPhoneRequestId, PublicIntent};

/// Exact allocated original, durably reserved before entropy or signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerPhoneReservation {
    id: OwnerPhoneRequestId,
    context: OwnerPhoneContext,
}
impl OwnerPhoneReservation {
    /// Preserve validated input; allocation and device authority are separate.
    pub fn new(id: OwnerPhoneRequestId, context: OwnerPhoneContext) -> Self {
        Self { id, context }
    }
    /// Exact originally allocated UUID.
    pub fn id(&self) -> OwnerPhoneRequestId {
        self.id
    }
    /// Exact original source and destination.
    pub fn context(&self) -> &OwnerPhoneContext {
        &self.context
    }
}

/// Private restored facts; the seed stays on the native side of the ABI.
pub struct OwnerPhonePrivateOriginal {
    reservation: OwnerPhoneReservation,
    key: Option<PrivateKeyMaterial>,
    public_intent: Option<PublicIntent>,
}
impl OwnerPhonePrivateOriginal {
    /// A reservation whose generating operation was not durably acknowledged.
    pub fn reserved(reservation: OwnerPhoneReservation) -> Self {
        Self {
            reservation,
            key: None,
            public_intent: None,
        }
    }
    /// Actual original seed and context from the same private record.
    pub fn prepared(reservation: OwnerPhoneReservation, key: PrivateKeyMaterial) -> Self {
        Self {
            reservation,
            key: Some(key),
            public_intent: None,
        }
    }
    /// Restore Prepared with its exact accepted public lineage. Consumers and
    /// the private codec validate correspondence; this constructs no authority.
    pub fn bound(
        reservation: OwnerPhoneReservation,
        key: PrivateKeyMaterial,
        public: PublicIntent,
    ) -> Self {
        Self {
            reservation,
            key: Some(key),
            public_intent: Some(public),
        }
    }
    /// Exact immutable lineage, absent until the explicit adoption owner binds it.
    pub fn public_intent(&self) -> Option<PublicIntent> {
        self.public_intent
    }
    /// Immutable original correlation.
    pub fn reservation(&self) -> &OwnerPhoneReservation {
        &self.reservation
    }
    /// Transfer the original seed to the native crypto owner without cloning it.
    pub fn into_parts(self) -> (OwnerPhoneReservation, Option<PrivateKeyMaterial>) {
        (self.reservation, self.key)
    }
    /// Native-only seed, absent for an incomplete reserved original.
    pub fn key(&self) -> Option<&PrivateKeyMaterial> {
        self.key.as_ref()
    }
}

/// Noncloneable, first-reservation operation. Dropping it leaves Reserved;
/// restoration never obtains another creation permit for that same record.
pub trait OwnerPhoneOriginalCreation {
    /// Original facts held under the same private operation owner.
    fn reservation(&self) -> &OwnerPhoneReservation;
    /// Publish the generated seed once. Success includes physical durability;
    /// expected failures retain the existing typed publication/effect evidence.
    fn prepare(self: Box<Self>, key: &PrivateKeyMaterial) -> Result<(), PrivateStateError>;
}
/// Native private storage, composed through FilePairingState's existing lifetime lock.
pub trait OwnerPhoneOriginalStore: Send + Sync {
    /// Reserve a new empty scope under its operation owner; reject legacy or
    /// existing originals before entropy. Success returns one live creation permit.
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError>;
    /// Restore actual original/seed without creating or replacing anything.
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError>;
    /// Whether the client's enrollment was adopted from a signed original, so
    /// that original must still be present. False with no enrollment, or for
    /// one redeemed by code, which never had an original.
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError>;
}
