//! Authorized record routing and bounded wire representation.
mod dispatch;
pub(crate) mod wire;
pub(super) use dispatch::dispatch;
