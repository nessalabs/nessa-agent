//! The dialing side of a peer gateway (slice H of ADR 252): this gateway
//! enrolls, with its own native key, into another gateway's peer invitation,
//! and keeps what it learned there, one private record per peer. Reading what
//! a peer granted is a later part; see `docs/design/auth/peer-gateways.md`.
//! `application` holds the evidence the owner's commands hand to audit.
pub mod application;
pub mod infrastructure;
