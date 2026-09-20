//! What the gateway context owns as rules rather than as effects.
//!
//! Today that is one value: the search path a process is given. The service is
//! registered with one for itself and carries a second, deliberately built one
//! for the agent, and deciding which strings may become either is a decision
//! with no launchd, no filesystem and no login shell in it.
//!
//! ```text
//! infrastructure::LoginShell ──raw shell output──▶ SearchPath::parse
//! application::Gateway ─────────SearchPath───────▶ infrastructure::Launchd (plist)
//! ```
//! Arrows mean values handed inward and then back out. Nothing here reads
//! anything from outside the process; the adapters on either end do that.
pub mod value_objects;
