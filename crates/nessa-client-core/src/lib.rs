//! Device-side enrollment and retained sync, independent of the gateway runtime.
//! `pairing` owns native enrollment; `composition` owns the example command API.
//! Retained sync/cache/reset implementations and their typed causes stay internal.
#![deny(missing_docs)]
pub mod composition;
pub mod pairing;
mod read_only_sync;
pub use composition::error::CommandError;
