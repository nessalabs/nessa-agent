//! Shared passive read transport conversion and capped response allocation.
//! Record and catalogue codecs consume this representation owner.
//! The product schema publishes phase budgets and the passive request floor.
//! `deadlines` converts generated phases to Durations for the socket; default
//! example composition consumes the generated floor for its client-owned deadline.
pub(crate) mod deadlines;
pub(crate) mod wire;
