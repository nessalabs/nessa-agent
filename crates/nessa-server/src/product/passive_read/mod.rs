//! Passive read socket deadlines, and the grant each read asks Cedar for.
//!
//! The shared transport conversion and capped response encoder both ends of
//! the socket use live in `nessa_protocol::product::passive_read`; the record
//! and catalogue codecs consume it there. The product schema publishes phase
//! budgets and the passive request floor. `deadlines` converts generated
//! phases to Durations for the socket; default example composition consumes
//! the generated floor for its client-owned deadline. `grants` answers the
//! application `PassiveReadGrants` port from the generated method mapping, so
//! admission does not import that mapping itself.
//!
//! ```text
//! manifest -> action_for_method -> grants -> PassiveReadGrants -> admission
//! ```
//!
//! Arrows are the grant's path into the read being admitted.
pub(crate) mod deadlines;
mod grants;
pub(crate) use grants::PUBLISHED_PASSIVE_READ_GRANTS;
