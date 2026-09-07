//! Identity and access library. No network server, provider SDK, or global state.
//! Domain invariants are independent of serialized application DTOs.
#![forbid(unsafe_code)]

pub mod application;
pub mod domain;

pub mod adapters;
