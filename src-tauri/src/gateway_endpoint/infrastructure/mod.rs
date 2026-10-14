//! Composes the shared private-file discovery adapter for the desktop host.

mod unavailable;

use crate::surface_credential::ServiceNamespace;
use nessa_gateway_endpoint::{
    application::EndpointDiscovery, infrastructure::FileEndpointDiscovery,
};
use std::sync::Arc;
use unavailable::UnavailableEndpointDiscovery;

/// Build discovery for composition's already-resolved service namespace.
pub fn endpoint_discovery(namespace: Option<ServiceNamespace>) -> Arc<dyn EndpointDiscovery> {
    match namespace {
        Some(namespace) => {
            let directory = namespace.logs_relative();
            Arc::new(FileEndpointDiscovery::new(namespace.root, directory))
        }
        None => Arc::new(UnavailableEndpointDiscovery),
    }
}
