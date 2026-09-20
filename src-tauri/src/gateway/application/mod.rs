//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
mod ports;
mod service;
#[cfg(test)]
pub(crate) mod testing;
pub use ports::{GatewayError, GatewayHost, LoginShellError, LoginShellPath, ReconciledGateway};
pub use service::Gateway;
