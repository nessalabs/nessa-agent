//! Public phone evidence mapping; fresh policy and signature verification stay in Auth.
use super::generated::{PhoneConsent, PhoneDiscovery, PhoneOriginal, PhoneOwnerSource};
use nessa_auth::{
    application::dto::{CredentialGrantDto, ResourceDto},
    domain::{
        pairing::{
            ConsentClass, DeviceKey, OwnerPhoneContext, OwnerPhoneRequest, OwnerPhoneRequestId,
            OwnerPhoneSource,
        },
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use std::net::SocketAddr;

/// Structural conversion refusal. None of these facts establish current authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhoneDtoError {
    /// Public byte array has another width than the receiving Auth type.
    Length,
    /// A wire integer is not an octet.
    Octet,
    /// Original identity is invalid or its gateway resource contradicts its audience.
    Source,
    /// Native destination is invalid or unusable according to Auth.
    Endpoint,
    /// Other immutable original value is invalid according to Auth.
    Original,
    /// Submitted consent differs from the canonical Auth publication.
    Consent,
}
/// Checked public bytes. Callers infer the fixed width from the consuming Auth API.
/// This is a representation conversion, not a key/signature/digest verification.
pub fn octets<const N: usize>(value: &[u64]) -> Result<[u8; N], PhoneDtoError> {
    if value.len() != N {
        return Err(PhoneDtoError::Length);
    }
    let mut bytes = [0; N];
    for (output, input) in bytes.iter_mut().zip(value) {
        *output = u8::try_from(*input).map_err(|_| PhoneDtoError::Octet)?;
    }
    Ok(bytes)
}
fn public_bytes(bytes: &[u8]) -> Vec<u64> {
    bytes.iter().copied().map(u64::from).collect()
}
fn canonical_consent(resource: &Resource) -> Result<PhoneConsent, PhoneDtoError> {
    let class = ConsentClass::OwnerPhoneReadWrite;
    let grants = class
        .grants(resource.clone())
        .map_err(|_| PhoneDtoError::Consent)?
        .into_iter()
        .map(|grant| CredentialGrantDto {
            action: grant.action().as_str().to_owned(),
            resource: ResourceDto {
                organization_id: grant.resource().organization_id().as_str().to_owned(),
                id: grant.resource().id().as_str().to_owned(),
            },
        })
        .collect();
    Ok(PhoneConsent {
        class: class.as_str().to_owned(),
        grants,
    })
}
/// Composition supplies this exact source, endpoint and public pin. This mapper
/// performs no discovery, fresh authorization or cryptographic pin validation.
pub fn discovery(
    source: &OwnerPhoneSource,
    endpoint: SocketAddr,
    pin: [u8; 44],
) -> Result<PhoneDiscovery, PhoneDtoError> {
    OwnerPhoneRequest::validate_endpoint(endpoint).map_err(|_| PhoneDtoError::Endpoint)?;
    Ok(PhoneDiscovery {
        source: PhoneOwnerSource {
            owner_id: source.owner().as_str().to_owned(),
            organization_id: source.resource().organization_id().as_str().to_owned(),
            membership_id: source.membership().as_str().to_owned(),
            audience: source.audience().as_str().to_owned(),
            credential_id: source.credential().as_str().to_owned(),
            resource_id: source.resource().id().as_str().to_owned(),
        },
        native_endpoint: endpoint.to_string(),
        gateway_key: public_bytes(&pin),
        consent: canonical_consent(source.resource())?,
    })
}
/// Preserve the exact stored original. It is historical correlation, not a new approval.
pub fn original(request: &OwnerPhoneRequest) -> Result<PhoneOriginal, PhoneDtoError> {
    Ok(PhoneOriginal {
        request_id: public_bytes(request.id().bytes()),
        discovery: discovery(request.source(), request.endpoint(), *request.gateway_pin())?,
        device_key: *request.device_key().bytes(),
    })
}
/// Validate discovery facts before any permanent device key exists. The route
/// still owns fresh current-source policy and signature verification.
pub fn context(discovery: PhoneDiscovery) -> Result<OwnerPhoneContext, PhoneDtoError> {
    let source = discovery.source;
    let resource = Resource::new(
        OrganizationId::new(source.organization_id).map_err(|_| PhoneDtoError::Source)?,
        ResourceId::new(source.resource_id).map_err(|_| PhoneDtoError::Source)?,
    );
    let source = OwnerPhoneSource::new(
        PrincipalId::new(source.owner_id).map_err(|_| PhoneDtoError::Source)?,
        MembershipId::new(source.membership_id).map_err(|_| PhoneDtoError::Source)?,
        AudienceId::new(source.audience).map_err(|_| PhoneDtoError::Source)?,
        CredentialId::new(source.credential_id).map_err(|_| PhoneDtoError::Source)?,
        resource,
    )
    .map_err(|_| PhoneDtoError::Source)?;
    let canonical = canonical_consent(source.resource())?;
    if discovery.consent.class != canonical.class || discovery.consent.grants != canonical.grants {
        return Err(PhoneDtoError::Consent);
    }
    let endpoint = discovery
        .native_endpoint
        .parse::<SocketAddr>()
        .map_err(|_| PhoneDtoError::Endpoint)?;
    OwnerPhoneContext::new(source, octets(&discovery.gateway_key)?, endpoint)
        .map_err(|_| PhoneDtoError::Endpoint)
}
/// Reconstruct Auth original using the same source/consent conversion as native preparation.
pub fn request(value: PhoneOriginal) -> Result<OwnerPhoneRequest, PhoneDtoError> {
    context(value.discovery)?
        .request(
            OwnerPhoneRequestId::new(octets(&value.request_id)?)
                .map_err(|_| PhoneDtoError::Original)?,
            DeviceKey::new(value.device_key),
        )
        .map_err(|_| PhoneDtoError::Original)
}

#[cfg(test)]
#[path = "../../tests/product/owner_phone.rs"]
mod tests;
