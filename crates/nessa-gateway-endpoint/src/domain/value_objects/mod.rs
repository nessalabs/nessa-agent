//! Immutable values that identify and advertise one bound local gateway.
//!
//! `endpoint` validates the listener and process identity. `advertisement` keeps
//! optional desktop-managed identity consistent with that endpoint.

mod advertisement;
mod endpoint;

pub use advertisement::{GatewayEndpointAdvertisement, ManagedRuntimeIdentity};
pub use endpoint::{EndpointIdentity, GatewayEndpoint};
