use nessa_gateway_endpoint::{application::EndpointDiscovery, domain::GatewayEndpoint};
use std::io;

pub(super) struct UnavailableEndpointDiscovery;

impl EndpointDiscovery for UnavailableEndpointDiscovery {
    fn discover(&self) -> io::Result<Option<GatewayEndpoint>> {
        Ok(None)
    }
}
