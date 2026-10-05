use super::super::{
    DeviceProofBinding, LocalCredentialStore, Registry, StoredCredential, StoredProof,
};
use crate::{
    application::{
        pairing::DeviceConnectionProof,
        ports::{
            AccessError, CredentialEvidence, CredentialVerifier, PortFuture, VerifiedCredential,
        },
    },
    domain::{AudienceId, CredentialId},
};
/// Credential verifier borrowing possession evidence from one live native TLS transport.
/// Raw credential IDs are accepted only with the transport's private proof and exact key binding.
pub struct DeviceCredentialVerifier<'a> {
    store: &'a LocalCredentialStore,
    proof: &'a DeviceConnectionProof,
}
impl LocalCredentialStore {
    /// Scope the current authentication contract to this live native device connection.
    /// The browser bearer verifier remains the other current proof mechanism.
    pub fn device_verifier<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
    ) -> DeviceCredentialVerifier<'a> {
        DeviceCredentialVerifier { store: self, proof }
    }
}
impl DeviceCredentialVerifier<'_> {
    /// Whether the key this connection proved holds some active (unrevoked)
    /// device credential for `audience`: the binding rule `verify` applies to
    /// one named credential, asked of every credential. It authenticates
    /// nothing; a caller that admits on it still runs the full
    /// authentication with the credential the peer then names.
    pub fn holds_credential(&self, audience: &AudienceId) -> Result<bool, AccessError> {
        self.store.with_published_registry(|registry| {
            for credential in &registry.credentials {
                match bound_to_proof(registry, credential, audience, self.proof) {
                    Ok(()) => return Ok(true),
                    Err(AccessError::InvalidCredential) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(false)
        })
    }
}
impl CredentialVerifier for DeviceCredentialVerifier<'_> {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            let id = std::str::from_utf8(evidence.expose_bytes())
                .map_err(|_| AccessError::InvalidCredential)?;
            self.store.with_published_registry(|registry| {
                let credential = registry
                    .credentials
                    .iter()
                    .find(|entry| entry.metadata.id == id)
                    .ok_or(AccessError::InvalidCredential)?;
                bound_to_proof(registry, credential, audience, self.proof)?;
                Ok(VerifiedCredential {
                    credential_id: CredentialId::new(id).map_err(|_| AccessError::Unavailable)?,
                    expires_at: credential.metadata.expires_at,
                })
            })
        })
    }
}

/// The one device binding rule: `credential` is for `audience`, unrevoked, a
/// device-pairing credential, and its enrollment claimed exactly the key
/// `proof` carries. `InvalidCredential` when it is not; `Unavailable` when its
/// enrollment cannot be read.
fn bound_to_proof(
    registry: &Registry,
    credential: &StoredCredential,
    audience: &AudienceId,
    proof: &DeviceConnectionProof,
) -> Result<(), AccessError> {
    if credential.metadata.audience_id != audience.as_str()
        || credential.metadata.revoked_at.is_some()
    {
        return Err(AccessError::InvalidCredential);
    }
    let StoredProof::Device(DeviceProofBinding::DevicePairing { invitation, .. }) =
        &credential.verifier
    else {
        return Err(AccessError::InvalidCredential);
    };
    let pairing = registry
        .pairings
        .iter()
        .find(|entry| entry.id().bytes() == invitation)
        .ok_or(AccessError::Unavailable)?;
    let record = pairing
        .restore(&registry.transitions)
        .map_err(|_| AccessError::Unavailable)?;
    // Registry validation also makes an unrevoked device credential Active.
    if record
        .claim_binding()
        .is_none_or(|(_, key)| key != proof.key())
    {
        return Err(AccessError::InvalidCredential);
    }
    Ok(())
}
