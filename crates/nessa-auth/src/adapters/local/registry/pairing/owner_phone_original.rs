//! Exact original input retained in the same registry entry as owner approval.
use crate::{
    application::pairing::VerifiedOwnerPhoneDeviceProof,
    domain::{
        pairing::{
            DeviceKey, OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource, PairingError,
        },
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct StoredOwnerPhoneOriginal {
    request: [u8; 16],
    owner: String,
    membership: String,
    audience: String,
    credential: String,
    organization: String,
    resource: String,
    gateway_pin: Vec<u8>,
    endpoint: SocketAddr,
    device_key: [u8; 32],
    digest: [u8; 32],
}
impl StoredOwnerPhoneOriginal {
    pub(super) fn from_proof(proof: &VerifiedOwnerPhoneDeviceProof) -> Self {
        let request = proof.request();
        let source = request.source();
        Self {
            request: *request.id().bytes(),
            owner: source.owner().as_str().into(),
            membership: source.membership().as_str().into(),
            audience: source.audience().as_str().into(),
            credential: source.credential().as_str().into(),
            organization: source.resource().organization_id().as_str().into(),
            resource: source.resource().id().as_str().into(),
            gateway_pin: request.gateway_pin().to_vec(),
            endpoint: request.endpoint(),
            device_key: *request.device_key().bytes(),
            digest: *proof.digest(),
        }
    }
    pub(super) fn restore(&self) -> Result<OwnerPhoneRequest, PairingError> {
        let invalid = |_| PairingError::Invalid;
        let source = OwnerPhoneSource::new(
            PrincipalId::new(self.owner.clone()).map_err(invalid)?,
            MembershipId::new(self.membership.clone()).map_err(invalid)?,
            AudienceId::new(self.audience.clone()).map_err(invalid)?,
            CredentialId::new(self.credential.clone()).map_err(invalid)?,
            Resource::new(
                OrganizationId::new(self.organization.clone()).map_err(invalid)?,
                ResourceId::new(self.resource.clone()).map_err(invalid)?,
            ),
        )?;
        let pin = self
            .gateway_pin
            .as_slice()
            .try_into()
            .map_err(|_| PairingError::Invalid)?;
        if crate::adapters::pairing::relay_key(&pin).is_none() {
            return Err(PairingError::Invalid);
        }
        let request = OwnerPhoneRequest::new(
            OwnerPhoneRequestId::new(self.request)?,
            source,
            pin,
            self.endpoint,
            DeviceKey::new(self.device_key),
        )?;
        if digest(&SHA256, &request.transcript()).as_ref() != self.digest {
            return Err(PairingError::Conflict);
        }
        Ok(request)
    }
    pub(super) fn digest(&self) -> [u8; 32] {
        self.digest
    }
}
