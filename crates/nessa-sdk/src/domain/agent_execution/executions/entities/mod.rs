//! Invocation evidence owns agreement between scheduling, observations, and settlement.
//!
//! application recording / restoration --> InvocationHistory
//!
//! The arrow means both paths apply the same pure domain rules.
mod invocation_history;
pub use invocation_history::{InvocationHistory, InvocationHistoryError, InvocationObservation};
