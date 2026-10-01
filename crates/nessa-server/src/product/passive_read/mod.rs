//! Shared passive read transport conversion and capped response allocation.
//! Record and catalogue codecs consume this representation owner.
//! `deadlines` publishes source/read and queued-delivery phases consumed by
//! the socket and default example composition; the operation deadline is client-owned.
pub(crate) mod deadlines;
pub(crate) mod wire;
