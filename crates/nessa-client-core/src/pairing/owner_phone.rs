//! Nonnetwork original-key preparation. Persistence, crypto and correlation have one owner each.
use nessa_auth::{
    adapters::pairing::{
        relay_key, verify_owner_phone_request, CryptoRng, NativeIdentity, PairingCryptoError,
        RngCore,
    },
    application::pairing::{
        OwnerPhoneOriginalStore, OwnerPhonePrivateOriginal, OwnerPhoneReservation,
        PrivateStateError,
    },
    domain::pairing::OwnerPhoneRequest,
};
use std::{
    error::Error,
    fmt::{Display, Formatter, Result as FmtResult},
};

/// Expected preparation refusal, preserving source failures without string parsing.
#[derive(Debug)]
pub enum OwnerPhonePreparationError {
    /// Immutable source/pin/key cannot construct the canonical signed original.
    Crypto(PairingCryptoError),
    /// Actual native persistence/refusal/effect evidence.
    Private(PrivateStateError),
    /// Existing app reservation has no native original; initial preparation is not a fallback.
    Missing,
    /// Reservation exists but its seed was never durably prepared; no replacement is generated.
    Incomplete,
}
impl Display for OwnerPhonePreparationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{self:?}")
    }
}
impl Error for OwnerPhonePreparationError {}

/// Public immutable proof. It is neither remote approval nor current private-key possession.
pub struct OwnerPhonePreparation {
    original: OwnerPhoneRequest,
    signature: [u8; 64],
    digest: [u8; 32],
}
impl OwnerPhonePreparation {
    /// Exact original values signed by the saved native seed.
    pub fn original(&self) -> &OwnerPhoneRequest {
        &self.original
    }
    /// Public original signature, not a bearer credential.
    pub fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
    /// Canonical digest computed by Auth's existing proof verifier.
    pub fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}
fn validate(reservation: &OwnerPhoneReservation) -> Result<(), OwnerPhonePreparationError> {
    relay_key(reservation.context().gateway_pin()).ok_or(OwnerPhonePreparationError::Crypto(
        PairingCryptoError::InvalidContext,
    ))?;
    Ok(())
}
fn proof(
    reservation: &OwnerPhoneReservation,
    identity: &NativeIdentity,
) -> Result<OwnerPhonePreparation, OwnerPhonePreparationError> {
    let original = reservation
        .context()
        .request(reservation.id(), identity.device_key())
        .map_err(|_| OwnerPhonePreparationError::Crypto(PairingCryptoError::InvalidContext))?;
    let signature = identity
        .sign_owner_phone_request(&original)
        .map_err(OwnerPhonePreparationError::Crypto)?;
    let verified = verify_owner_phone_request(original.clone(), &signature)
        .map_err(OwnerPhonePreparationError::Crypto)?;
    Ok(OwnerPhonePreparation {
        original,
        signature,
        digest: *verified.digest(),
    })
}
/// Initial allocation only. A live first-reservation operation durably fences
/// the original before entropy, and retains its private lease through seed acknowledgement.
pub fn prepare_owner_phone(
    store: &dyn OwnerPhoneOriginalStore,
    reservation: OwnerPhoneReservation,
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<OwnerPhonePreparation, OwnerPhonePreparationError> {
    validate(&reservation)?;
    if let Some(saved) = store
        .load_original()
        .map_err(OwnerPhonePreparationError::Private)?
    {
        return restored_proof(&reservation, saved);
    }
    let creation = store
        .reserve_original(reservation.clone())
        .map_err(OwnerPhonePreparationError::Private)?;
    if creation.reservation() != &reservation {
        return Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Conflict,
        ));
    }
    let identity = NativeIdentity::generate(rng).map_err(OwnerPhonePreparationError::Crypto)?;
    creation
        .prepare(identity.key_material())
        .map_err(OwnerPhonePreparationError::Private)?;
    proof(&reservation, &identity)
}
/// Existing reservations restore only; missing or incomplete history never causes entropy or replacement.
pub fn restore_owner_phone(
    store: &dyn OwnerPhoneOriginalStore,
    reservation: &OwnerPhoneReservation,
) -> Result<OwnerPhonePreparation, OwnerPhonePreparationError> {
    validate(reservation)?;
    let saved = store
        .load_original()
        .map_err(OwnerPhonePreparationError::Private)?
        .ok_or(OwnerPhonePreparationError::Missing)?;
    restored_proof(reservation, saved)
}
fn restored_proof(
    reservation: &OwnerPhoneReservation,
    saved: OwnerPhonePrivateOriginal,
) -> Result<OwnerPhonePreparation, OwnerPhonePreparationError> {
    if saved.reservation() != reservation {
        return Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Conflict,
        ));
    }
    let (_, key) = saved.into_parts();
    let key = key.ok_or(OwnerPhonePreparationError::Incomplete)?;
    let identity = NativeIdentity::restore(key).map_err(OwnerPhonePreparationError::Crypto)?;
    proof(reservation, &identity)
}

#[cfg(test)]
#[path = "../../tests/pairing/owner_phone.rs"]
mod tests;
