//! Device-side enrollment and retained sync, independent of the gateway runtime.
//! `pairing` owns native enrollment; `composition` owns the example command API.
//! Retained sync/cache/reset implementations and their typed causes stay internal.
//! The `cli` feature (on by default) carries the example's command line and the
//! retained sync it composes; a gateway linking this crate as a peer's client
//! builds without it, so argument parsing never reaches the gateway.
#![deny(missing_docs)]
#[cfg(feature = "cli")]
pub mod composition;
pub mod pairing;
#[cfg(feature = "cli")]
mod read_only_sync;
#[cfg(feature = "cli")]
pub use composition::error::CommandError;
