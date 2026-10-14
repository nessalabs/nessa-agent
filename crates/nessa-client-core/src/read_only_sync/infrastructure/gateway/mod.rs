//! One protected native connection; operation deadlines cover each physical syscall.
//! `deadline_stream` owns cancellation, first typed failure and checks after I/O.
//! Enrollment uses `nessa_protocol::pairing::socket` instead: its phase/wake loop
//! retries tick-sized reads and transfers to buffered I/O for the gateway.
//! These distinct policies share the monotonic clock port, not a second owner.
//! `held_files` reads `artifact.read` ranges on the same connection, one
//! operation per range.
mod deadline_stream;
mod held_files;
mod session;
pub(crate) use session::{
    subscription_refusal, DeviceEvidence, LocalConnector, Session, SubscriptionDelivery,
};
mod sources;
pub(crate) use sources::{
    CatalogueReader, GatewayAuthorizer, GatewayConnection, RecordGatewaySource,
};

// The native peer harness is also what composition's `fetch` tests dial.
#[cfg(test)]
#[path = "../../../../tests/read_only_sync/gateway.rs"]
pub(crate) mod tests;
