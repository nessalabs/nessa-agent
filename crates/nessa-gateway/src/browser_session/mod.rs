//! Browser sign-in sessions. HTTP adapters own cookies; domain session state retains
//! the verified credential binding, exact structural origin, and idle lifetime,
//! never the submitted long-lived access token.
pub mod adapters;
pub mod application;
pub mod domain;
pub mod entrypoint;
