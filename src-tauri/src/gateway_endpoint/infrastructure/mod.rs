//! Composes the shared private-file discovery adapter for the desktop host.

mod unavailable;

use nessa_gateway_endpoint::{
    application::EndpointDiscovery, infrastructure::FileEndpointDiscovery,
};
use std::{path::PathBuf, sync::Arc};
use unavailable::UnavailableEndpointDiscovery;

/// Build discovery for composition's already-resolved service namespace.
pub fn endpoint_discovery(namespace: Option<PathBuf>) -> Arc<dyn EndpointDiscovery> {
    match namespace {
        Some(root) => Arc::new(FileEndpointDiscovery::new(root.join("logs"))),
        None => Arc::new(UnavailableEndpointDiscovery),
    }
}
