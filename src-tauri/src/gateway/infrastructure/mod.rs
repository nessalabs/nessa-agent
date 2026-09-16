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
//! ```text
//! Gateway -> Launchd -> staging -> verified immutable runtime
//!                    -> control -> launchd / existing gateway
//! ```
//! Arrows mean calls; only launchd owns the background process lifetime.
use super::application::GatewayHost;
use std::sync::Arc;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;
pub fn current() -> Arc<dyn GatewayHost> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::Launchd)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(unsupported::Unsupported)
    }
}
