//! Authorized record routing and bounded wire representation.
//! Dispatch and socket admission share the exhaustive read-refusal wire conversion.
mod dispatch;
pub(crate) mod wire;
pub(super) use dispatch::dispatch;
