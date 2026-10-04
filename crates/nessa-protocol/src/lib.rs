//! The contract between the Nessa gateway and its clients.
//!
//! Everything here is what both ends of a gateway connection need to agree on,
//! and nothing that only one end does: wire frames and the payloads generated
//! from the protocol schemas, the product contract's outcome values, and the
//! product DTOs. The gateway (`nessa-server`) depends on this crate; so does a
//! device client. This crate depends on neither.
//!
//! ```text
//! nessa-server ──▶ nessa-protocol ◀── device client
//! ```
//! Arrows are compile-time dependencies.
pub mod product;
pub mod product_contract;
pub mod protocol;
