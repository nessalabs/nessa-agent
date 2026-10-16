//! Immutable original request; values describe correlation, never current authority.
use super::{ConsentClass, ConsentIntent, DeviceKey, DisclosedConsent, PairingError};
use crate::domain::{AudienceId, CredentialId, Grant, MembershipId, PrincipalId, Resource};
use std::net::{IpAddr, SocketAddr};

/// Original owner-allocated setup UUID, retained across every lost reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OwnerPhoneRequestId([u8; 16]);
impl OwnerPhoneRequestId {
    /// Preserve one allocated request identity; all-zero is not an allocated UUID.
    pub fn new(bytes: [u8; 16]) -> Result<Self, PairingError> {
        if bytes == [0; 16] {
            return Err(PairingError::Invalid);
        }
        Ok(Self(bytes))
    }
    /// Exact original request bytes.
    pub fn bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Exact original bootstrap linkage. Construction grants no access; the server
/// derives this from its current authenticated session and composed resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerPhoneSource {
    owner: PrincipalId,
    membership: MembershipId,
    audience: AudienceId,
    credential: CredentialId,
    resource: Resource,
}
impl OwnerPhoneSource {
    /// Preserve the complete original identity without accepting another gateway audience.
    pub fn new(
        owner: PrincipalId,
        membership: MembershipId,
        audience: AudienceId,
        credential: CredentialId,
        resource: Resource,
    ) -> Result<Self, PairingError> {
        if resource.id().as_str() != audience.as_str() {
            return Err(PairingError::Invalid);
        }
        Ok(Self {
            owner,
            membership,
            audience,
            credential,
            resource,
        })
    }
    /// Original owner, also the delegated conversation owner.
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    /// Original membership, never replaced by enrollment.
    pub fn membership(&self) -> &MembershipId {
        &self.membership
    }
    /// Original gateway audience.
    pub fn audience(&self) -> &AudienceId {
        &self.audience
    }
    /// Exact original bootstrap credential; another owner credential cannot replay it.
    pub fn credential(&self) -> &CredentialId {
        &self.credential
    }
    /// Composition-resolved gateway and organization.
    pub fn resource(&self) -> &Resource {
        &self.resource
    }
}

/// Canonical signed owner-phone request. The signature proves this device key,
/// while fresh owner policy and registry publication remain separate decisions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerPhoneRequest {
    id: OwnerPhoneRequestId,
    source: OwnerPhoneSource,
    gateway_pin: [u8; 44],
    endpoint: SocketAddr,
    device_key: DeviceKey,
    capabilities: Vec<Grant>,
}
impl OwnerPhoneRequest {
    /// Capture the authenticated advertised numeric endpoint and exact permanent key.
    /// The cryptographic adapter independently validates the gateway's SPKI grammar.
    pub fn new(
        id: OwnerPhoneRequestId,
        source: OwnerPhoneSource,
        gateway_pin: [u8; 44],
        endpoint: SocketAddr,
        device_key: DeviceKey,
    ) -> Result<Self, PairingError> {
        Self::validate_endpoint(endpoint)?;
        let capabilities = ConsentClass::OwnerPhoneReadWrite.grants(source.resource.clone())?;
        Ok(Self {
            id,
            source,
            gateway_pin,
            endpoint,
            device_key,
            capabilities,
        })
    }
    /// Validate the authenticated advertised numeric destination before a
    /// permanent device key exists. `new` uses this same endpoint owner.
    /// A unicast host excludes IPv4 this-network/broadcast and IPv6 link-local:
    /// a gateway's numeric interface scope has no meaning on a remote phone.
    /// Concrete loopback endpoints remain usable for local clients.
    pub fn validate_endpoint(endpoint: SocketAddr) -> Result<(), PairingError> {
        let address = endpoint.ip().to_canonical();
        let unicast_host = match address {
            IpAddr::V4(address) => address.octets()[0] != 0 && !address.is_broadcast(),
            IpAddr::V6(address) => !address.is_unicast_link_local(),
        };
        if endpoint.port() == 0
            || address.is_unspecified()
            || address.is_multicast()
            || !unicast_host
        {
            return Err(PairingError::Invalid);
        }
        Ok(())
    }
    /// Immutable original request identity.
    pub fn id(&self) -> OwnerPhoneRequestId {
        self.id
    }
    /// Exact original identity linkage.
    pub fn source(&self) -> &OwnerPhoneSource {
        &self.source
    }
    /// Immutable gateway TLS pin.
    pub fn gateway_pin(&self) -> &[u8; 44] {
        &self.gateway_pin
    }
    /// Authenticated advertised endpoint, never inferred from a product host.
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    /// Permanent key independently verified by this request's signature.
    pub fn device_key(&self) -> DeviceKey {
        self.device_key
    }
    /// Compare complete disclosed scope through the canonical consent owner.
    /// Public invitation/attempt/expiry remain the separately retained lineage.
    pub fn correlate_disclosure(&self, disclosure: &DisclosedConsent) -> Result<(), PairingError> {
        let public = disclosure.public();
        let intent = ConsentIntent::new(
            public.consent(),
            public.generation(),
            self.source.audience.clone(),
            self.source.owner.clone(),
            self.source.membership.clone(),
            self.source.resource.clone(),
            ConsentClass::OwnerPhoneReadWrite,
        )?;
        disclosure.verify_intent(&intent)
    }
    /// One domain-separated, length-delimited signing transcript. No secret,
    /// server-generated invitation identity or ambient clock changes replay identity.
    pub fn transcript(&self) -> Vec<u8> {
        let mut bytes = b"nessa-owner-phone-authorization-v1\0".to_vec();
        fn field(bytes: &mut Vec<u8>, value: &[u8]) {
            bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
            bytes.extend_from_slice(value);
        }
        field(&mut bytes, self.id.bytes());
        for value in [
            self.source.owner.as_str(),
            self.source.resource.organization_id().as_str(),
            self.source.membership.as_str(),
            self.source.audience.as_str(),
            self.source.credential.as_str(),
            self.source.resource.id().as_str(),
        ] {
            field(&mut bytes, value.as_bytes());
        }
        field(&mut bytes, &self.gateway_pin);
        field(&mut bytes, self.endpoint.to_string().as_bytes());
        field(&mut bytes, self.device_key.bytes());
        field(
            &mut bytes,
            ConsentClass::OwnerPhoneReadWrite.as_str().as_bytes(),
        );
        // The capability owner supplies the whole ordered set, never caller-provided actions.
        for grant in &self.capabilities {
            field(&mut bytes, grant.action().as_str().as_bytes());
            field(
                &mut bytes,
                grant.resource().organization_id().as_str().as_bytes(),
            );
            field(&mut bytes, grant.resource().id().as_str().as_bytes());
        }
        bytes
    }
}
