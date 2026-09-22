//! What the gateway context owns as rules rather than as effects.
//!
//! These are the search path a process is given and the durable configuration
//! that identifies a packaged service. Deciding which raw values may become
//! either is a decision with no launchd, filesystem, or login shell in it.
//!
//! ```text
//! infrastructure::LoginShell ──raw shell output──▶ SearchPath::parse
//! validated inputs ──▶ ServiceConfiguration ──▶ future reconciliation wiring
//! application::Gateway ─────────validated values─────▶ infrastructure::Launchd
//! ```
//! Arrows mean values handed inward and then back out. Nothing here reads
//! anything from outside the process; the adapters on either end do that.
pub mod value_objects;
