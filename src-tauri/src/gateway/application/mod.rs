//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
mod ports;
mod service;
pub use ports::{GatewayError, GatewayHost};
pub use service::Gateway;
