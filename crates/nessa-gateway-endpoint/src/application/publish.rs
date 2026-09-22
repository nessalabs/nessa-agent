use super::{EndpointDiscovery, EndpointPublication};
use crate::domain::GatewayEndpoint;
use std::io;

/// Existing launchd-managed identity projected into endpoint publication.
///
/// Composition obtains these values from the already validated desktop runtime;
/// this context does not reinterpret or fabricate them for standalone servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedRuntimeAdvertisement {
    pub(crate) fingerprint: String,
    pub(crate) generation: String,
    pub(crate) instance: String,
    pub(crate) process_id: u32,
}

impl ManagedRuntimeAdvertisement {
    pub fn new(
        fingerprint: String,
        generation: String,
        instance: String,
        process_id: u32,
        endpoint: &crate::domain::EndpointIdentity,
    ) -> Result<Self, &'static str> {
        let digest = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        };
        if !digest(&fingerprint) || !digest(&generation) {
            return Err("invalid managed runtime digest");
        }
        if instance != endpoint.instance() || process_id != endpoint.process_id() {
            return Err("managed runtime and endpoint identity disagree");
        }
        Ok(Self {
            fingerprint,
            generation,
            instance,
            process_id,
        })
    }
}

/// Resolve a trustworthy publication before a caller loads credentials.
pub struct DiscoverGatewayEndpoint<'a> {
    discovery: &'a dyn EndpointDiscovery,
}

impl<'a> DiscoverGatewayEndpoint<'a> {
    pub fn new(discovery: &'a dyn EndpointDiscovery) -> Self {
        Self { discovery }
    }

    pub fn execute(&self) -> io::Result<Option<GatewayEndpoint>> {
        self.discovery.discover()
    }
}

/// Publish one already-bound endpoint.
pub struct PublishGatewayEndpoint<'a> {
    publication: &'a dyn EndpointPublication,
}

impl<'a> PublishGatewayEndpoint<'a> {
    pub fn new(publication: &'a dyn EndpointPublication) -> Self {
        Self { publication }
    }

    pub fn execute(
        &self,
        endpoint: &GatewayEndpoint,
        managed: Option<&ManagedRuntimeAdvertisement>,
    ) -> io::Result<()> {
        self.publication.publish(endpoint, managed)
    }
}
