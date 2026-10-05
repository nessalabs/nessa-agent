//! Passive read socket deadlines.
//!
//! The shared transport conversion and capped response encoder both ends of
//! the socket use live in `nessa_protocol::product::passive_read`; the record
//! and catalogue codecs consume it there. The product schema publishes phase
//! budgets and the passive request floor. `deadlines` converts generated
//! phases to Durations for the socket; default example composition consumes
//! the generated floor for its client-owned deadline.
pub(crate) mod deadlines;
