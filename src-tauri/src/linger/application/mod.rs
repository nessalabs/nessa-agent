//! The setup choice: read logind, and call it only after an explicit accept.
//!
//! ```text
//! status ──▶ read ──▶ show
//! accept ──▶ read ──▶ SetUserLinger, when linger is off ──▶ read ──▶ show
//! ```

mod session;

pub(crate) use session::LingerOffer;
#[cfg(not(target_os = "linux"))]
pub(crate) use session::NotApplicableLinger;
#[cfg(any(test, target_os = "linux"))]
pub(crate) use session::{LingerSession, LogindLinger};
