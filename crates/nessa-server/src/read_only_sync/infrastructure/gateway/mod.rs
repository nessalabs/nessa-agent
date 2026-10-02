//! One synchronous socket; absolute deadlines cover each physical syscall.
mod deadline_stream;
mod session;
pub(crate) use session::{LocalConnector, Session};
mod sources;
pub(crate) use sources::GatewayConnection;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/gateway.rs"]
mod tests;
