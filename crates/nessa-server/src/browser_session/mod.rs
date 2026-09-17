//! Browser sign-in sessions. HTTP adapters own cookies; application sessions retain
//! verified identity, never the submitted long-lived access token.
pub mod adapters;
pub mod application;
pub mod domain;
pub mod entrypoint;
