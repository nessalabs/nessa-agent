//! What a credential was issued to comes from its stored proof: a bearer
//! token, or the pairing its device key was issued under.
use super::super::{DeviceProofBinding, LocalCredentialStore, StoredProof};
use crate::{
    application::{
        pairing::{CredentialDevice, CredentialDeviceSource},
        ports::{AccessError, PortFuture},
    },
    domain::{pairing::ConsentClass, CredentialId},
};
impl CredentialDeviceSource for LocalCredentialStore {
    fn credential_device<'a>(
        &'a self,
        credential: &'a CredentialId,
    ) -> PortFuture<'a, Option<CredentialDevice>> {
        Box::pin(async move {
            self.with_published_registry(|registry| {
                let Some(stored) = registry
                    .credentials
                    .iter()
                    .find(|entry| entry.metadata.id == credential.as_str())
                else {
                    return Ok(None);
                };
                let StoredProof::Device(DeviceProofBinding::DevicePairing { invitation, .. }) =
                    &stored.verifier
                else {
                    return Ok(Some(CredentialDevice::Token));
                };
                let record = registry
                    .pairings
                    .iter()
                    .find(|entry| entry.id().bytes() == invitation)
                    .ok_or(AccessError::Unavailable)?
                    .restore(&registry.transitions)
                    .map_err(|_| AccessError::Unavailable)?;
                Ok(Some(match record.intent().class() {
                    ConsentClass::OwnerPhoneReadWrite => CredentialDevice::Phone,
                    ConsentClass::DeviceRead => CredentialDevice::PairedDevice,
                    ConsentClass::PeerRead | ConsentClass::PeerEnvironment => {
                        CredentialDevice::Peer
                    }
                }))
            })
        })
    }
}
