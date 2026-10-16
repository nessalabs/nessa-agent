//! Separate signed-owner proof; it cannot construct a PAKE or TLS verdict.
use super::{relay_key, NativeIdentity, PairingCryptoError};
use crate::{
    application::pairing::VerifiedOwnerPhoneDeviceProof, domain::pairing::OwnerPhoneRequest,
};
use ring::{
    digest::{digest, SHA256},
    signature::{Ed25519KeyPair, UnparsedPublicKey, ED25519},
};

impl NativeIdentity {
    /// Sign only the canonical owner-phone request for this permanent device key.
    /// The native caller persists this exact input before any product mutation.
    pub fn sign_owner_phone_request(
        &self,
        request: &OwnerPhoneRequest,
    ) -> Result<[u8; 64], PairingCryptoError> {
        validate(request)?;
        if request.device_key() != self.device_key() {
            return Err(PairingCryptoError::InvalidContext);
        }
        let pair = Ed25519KeyPair::from_seed_unchecked(self.key_material().expose_bytes())
            .map_err(|_| PairingCryptoError::Unavailable)?;
        let signature = pair.sign(&request.transcript());
        let mut bytes = [0; 64];
        bytes.copy_from_slice(signature.as_ref());
        Ok(bytes)
    }
}
fn validate(request: &OwnerPhoneRequest) -> Result<(), PairingCryptoError> {
    relay_key(request.gateway_pin()).ok_or(PairingCryptoError::InvalidContext)?;
    Ok(())
}
/// Verify the actual key signature on the server's expected original input.
/// No caller-supplied scope can replace the authenticated source/composed endpoint.
pub fn verify_owner_phone_request(
    request: OwnerPhoneRequest,
    signature: &[u8; 64],
) -> Result<VerifiedOwnerPhoneDeviceProof, PairingCryptoError> {
    validate(&request)?;
    let transcript = request.transcript();
    UnparsedPublicKey::new(&ED25519, request.device_key().bytes())
        .verify(&transcript, signature)
        .map_err(|_| PairingCryptoError::InvalidProof)?;
    let mut fingerprint = [0; 32];
    fingerprint.copy_from_slice(digest(&SHA256, &transcript).as_ref());
    Ok(VerifiedOwnerPhoneDeviceProof::from_signature(
        request,
        fingerprint,
    ))
}
