//! One protected native connection; absolute deadlines cover each physical syscall.
mod deadline_stream;
mod session;
pub(crate) use session::{DeviceEvidence, LocalConnector, Session};
mod sources;
pub(crate) use sources::{GatewayAuthorizer, GatewayConnection, RecordGatewaySource};

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/gateway.rs"]
mod tests;
