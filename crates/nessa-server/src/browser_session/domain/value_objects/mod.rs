//! Immutable validated idle deadlines. Renewal produces a replacement value.
mod lifetime;
pub use lifetime::{Lifetime, FUTURE_TOLERANCE_SECONDS, IDLE_SECONDS};
mod removal_reason;
pub use removal_reason::RemovalReason;
