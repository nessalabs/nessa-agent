//! Immutable validated values owned by the browser-session domain.
//!
//! Request/persistence text -> browser-session values -> application orchestration
//!
//! The arrows show untrusted boundary data becoming domain-owned values before
//! application or storage logic can retain it.
mod browser_session_origin;
mod lifetime;
mod removal_reason;
mod session_state;

pub use browser_session_origin::BrowserSessionOrigin;
pub use lifetime::{Lifetime, FUTURE_TOLERANCE_SECONDS, IDLE_SECONDS};
pub use removal_reason::RemovalReason;
pub use session_state::BrowserSessionState;
