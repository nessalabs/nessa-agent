use crate::domain::{GatewayEndpoint, GatewayEndpointAdvertisement};
use std::io;

/// Durable publication needed by the endpoint use case.
pub trait EndpointPublication: Send + Sync {
    fn publish(&self, advertisement: &GatewayEndpointAdvertisement) -> io::Result<()>;
}

/// Read and correlate the current endpoint. `None` means no publication could
/// be read, so each caller retains its own existing fallback policy.
pub trait EndpointDiscovery: Send + Sync {
    fn discover(&self) -> io::Result<Option<GatewayEndpoint>>;
}
