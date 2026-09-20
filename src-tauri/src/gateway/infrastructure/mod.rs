//! Native background service adapters. The desktop composition root selects one.
//! macOS reconciliation holds a service-label lock through retirement, replacement,
//! matching process readiness. Failed installation preserves the desired registration
//! for forward recovery; it never rolls back or stops an unretired replacement. Its `control` module owns private retirement
//! exchange files, bounded health parsing, and launchctl effects. Its `generation`
//! module reuses the published generation only for an equal unfenced definition;
//! changed or retired definitions receive a fresh random installation identity.
//! `staging` verifies and publishes private immutable runtime versions before any
//! service mutation. Existing versions remain available across app replacement.
//!
//! `login_shell` is the other native read here and belongs to no service: it asks
//! the account's own login shell for the search path the agent will be given.
//!
//! ```text
//! Gateway -> Launchd -> staging -> verified immutable runtime
//!                    -> control -> launchd / existing gateway
//!         -> LoginShell -------> the account's login shell
//! ```
//! Arrows mean calls; only launchd owns the background process lifetime.
mod login_shell;
#[cfg(target_os = "macos")]
mod macos;
mod selection;
#[cfg(not(target_os = "macos"))]
mod unsupported;
pub use selection::{current, login_shell_path};
