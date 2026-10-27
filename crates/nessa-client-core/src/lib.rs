//! Device-side enrollment, retained sync, and the phone send outbox, independent
//! of the gateway runtime.
//! `pairing` owns native enrollment; `retained` reads an owner's or granted
//! peer's records, projects saved evidence and composes enrollment with purging;
//! `outbox` keeps one typed phone send until its outcome is known;
//! `composition` owns the example API.
//! Retained sync/cache/reset implementations and their typed causes stay internal.
//! The `cli` feature (on by default) carries the example's command line and
//! its presentation; a gateway linking this crate as a peer's client builds
//! without it, so argument parsing never reaches the gateway, while the sync
//! core `retained` calls is built either way.
#![deny(missing_docs)]
#[cfg(feature = "cli")]
pub mod composition;
pub mod outbox;
pub mod pairing;
pub mod product;
mod protected;
mod read_only_sync;
pub mod retained;
pub mod tunnel;
#[cfg(feature = "cli")]
pub use composition::error::CommandError;
