//! What the gateway context owns as rules rather than as effects.
//!
//! These are the search path a process is given and immutable evidence for why
//! service reconciliation was requested and by whom. Their construction has no
//! launchd, filesystem, framework, clock, or randomness.
//!
//! ```text
//! infrastructure::LoginShell ──raw shell output──▶ SearchPath::parse
//! application::Gateway ─────────SearchPath───────▶ infrastructure::Launchd (plist)
//! application::Gateway ──request/attempt evidence▶ infrastructure audit
//! ```
//! Arrows mean values handed inward and then back out. Nothing here reads
//! anything from outside the process; the adapters on either end do that.
pub mod value_objects;
