//! Device-side enrollment and retained sync, independent of the gateway runtime.
//! `pairing` owns native enrollment; `retained` reads what a paired reader was
//! granted into its retained cache; `composition` owns the example command API.
//! Retained sync/cache/reset implementations and their typed causes stay internal.
//! The `cli` feature (on by default) carries the example's command line and
//! its presentation; a gateway linking this crate as a peer's client builds
//! without it, so argument parsing never reaches the gateway, while the sync
//! core `retained` calls is built either way.
#![deny(missing_docs)]
#[cfg(feature = "cli")]
pub mod composition;
pub mod pairing;
mod read_only_sync;
pub mod retained;
#[cfg(feature = "cli")]
pub use composition::error::CommandError;
