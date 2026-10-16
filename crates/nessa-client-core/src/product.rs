//! Bounded protected product requests over the canonical pinned native handshake.
//! One physical pump owns TLS and preserves original request correlation. This
//! transport exposes actual ready metadata; it does not grant product authority.
mod ports;
mod session;
mod worker;
pub use ports::{
    ConnectedProduct, LocalProductConnector, ProductConnector, ProductInterrupt, ProductStream,
    ProductWake, ThreadProductWake,
};
pub use session::{
    ProductCompletion, ProductEventBatch, ProductFailure, ProductLimits, ProductSession,
    ProductSettlement, ProductTicket,
};

#[cfg(test)]
#[path = "../tests/product/mod.rs"]
mod tests;
