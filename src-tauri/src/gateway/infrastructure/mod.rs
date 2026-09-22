//! Native background service adapters. The desktop composition root selects one.
//! macOS reconciliation holds a service-label lock through retirement, replacement,
//! matching process readiness. Failed installation preserves the desired registration
//! for forward recovery; it never rolls back or stops an unretired replacement. Its `control` module owns private retirement
//! exchange files, bounded health parsing, and launchctl effects. Its `generation`
//! module reuses the published generation only for an equal unfenced definition;
//! changed or retired definitions receive a fresh random installation identity.
//! `staging` verifies and publishes private immutable runtime versions before any
//! service mutation. Existing versions remain available across app replacement.
//! Its `pruning` module then collects, under the same lock and only once a
//! gateway is up, the versions neither the new service, the loaded service, nor
//! an outstanding retirement can still need; it never fails registration.
//!
//! `login_shell` is the other native read here and belongs to no service: it asks
//! the account's own login shell for the search path the agent will be given.
//!
//! ```text
//! bundled windows -> commands -> Gateway startup snapshot / retry
//! Gateway -> Launchd -> staging -> verified immutable runtime
//!                    -> control -> launchd / existing gateway
//!                    -> pruning -> superseded runtime versions
//!         -> reconciliation audit -> private atomic intent/outcome records
//!         -> LoginShell -------> the account's login shell
//! ```
//! Arrows mean calls; commands translate only the application-owned startup
//! contract, and only launchd owns the background process lifetime.
mod commands;
mod login_shell;
#[cfg(target_os = "macos")]
mod macos;
mod reconciliation_ids;
mod selection;
#[cfg(not(target_os = "macos"))]
mod unsupported;
pub use commands::{gateway_startup, retry_gateway_startup, startup_events};
pub use selection::{current, login_shell_path, reconciliation_audit, reconciliation_ids};
