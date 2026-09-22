use super::{EndpointDiscovery, EndpointPublication};
use crate::domain::{GatewayEndpoint, GatewayEndpointAdvertisement};
use std::io;

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

    pub fn execute(&self, advertisement: &GatewayEndpointAdvertisement) -> io::Result<()> {
        self.publication.publish(advertisement)
    }
}
