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
//! ```text
//! Gateway -> Launchd -> staging -> verified immutable runtime
//!                    -> control -> launchd / existing gateway
//!                    -> pruning -> superseded runtime versions
//! ```
//! Arrows mean calls; only launchd owns the background process lifetime.
#[cfg(target_os = "macos")]
mod macos;
mod selection;
#[cfg(not(target_os = "macos"))]
mod unsupported;
pub use selection::current;
