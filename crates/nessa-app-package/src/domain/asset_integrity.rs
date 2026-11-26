use nessa_types::app_identity::ExtensionAppId;
use sha2::{Digest, Sha256};

/// The observed bytes did not match their expected integrity metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetIntegrityError {
    /// The byte count differed from the expected count.
    LengthMismatch,
    /// The byte count matched, but the SHA-256 digest differed.
    DigestMismatch,
}

/// A borrowed asset and the app identity supplied alongside its expected metadata.
///
/// Verifying bytes does not establish package provenance or authorize the app.
#[derive(Debug)]
pub struct VerifiedAsset<'a> {
    app_id: &'a ExtensionAppId,
    bytes: &'a [u8],
}

impl<'a> VerifiedAsset<'a> {
    /// Borrow the caller-supplied, validated extension app identity.
    pub fn app_id(&self) -> &'a ExtensionAppId {
        self.app_id
    }

    /// Borrow the exact bytes checked by the verifier.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// Verify held bytes against the expected byte count and binary SHA-256 digest.
///
/// The caller owns the read and its size limit. Use the returned bytes directly
/// to avoid substituting a fresh read for the checked asset.
///
/// # Errors
///
/// Returns a typed length or digest mismatch. Length is checked before hashing.
pub fn verify_asset<'a>(
    app_id: &'a ExtensionAppId,
    bytes: &'a [u8],
    expected_byte_count: u64,
    expected_sha256: [u8; 32],
) -> Result<VerifiedAsset<'a>, AssetIntegrityError> {
    if u64::try_from(bytes.len()) != Ok(expected_byte_count) {
        return Err(AssetIntegrityError::LengthMismatch);
    }
    let actual_sha256: [u8; 32] = Sha256::digest(bytes).into();
    if actual_sha256 != expected_sha256 {
        return Err(AssetIntegrityError::DigestMismatch);
    }

    Ok(VerifiedAsset { app_id, bytes })
}
