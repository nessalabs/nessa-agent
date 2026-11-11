//! Durable recovery state for installation audit delivery.
//!
//! The retained lock fences recovery and publication admission. Record mapping
//! is separate from the retained-directory I/O that publishes each immutable
//! preparation, outcome, and settlement.

mod journal;
mod record;

pub use journal::DurableInstallationDelivery;
