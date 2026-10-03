//! Concrete identity and access adapters selected by application composition.

pub mod cedar;
pub mod local;

/// Selected native TLS and PAKE enrollment profile.
pub mod pairing;
