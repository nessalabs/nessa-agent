//! One protected native connection; operation deadlines cover each physical syscall.
//! `deadline_stream` owns cancellation, first typed failure and checks after I/O.
//! Enrollment uses `nessa_protocol::pairing::socket` instead: its phase/wake loop
//! retries tick-sized reads and transfers to buffered I/O for the gateway.
//! These distinct policies share the monotonic clock port, not a second owner.
mod deadline_stream;
mod session;
pub(crate) use session::{DeviceEvidence, LocalConnector, Session};
mod sources;
pub(crate) use sources::{
    CatalogueReader, GatewayAuthorizer, GatewayConnection, RecordGatewaySource,
};

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/gateway.rs"]
mod tests;
