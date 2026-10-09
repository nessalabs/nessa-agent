//! Native background service adapters. The desktop composition root selects launchd
//! on macOS and the account's systemd user manager on Linux. macOS reconciliation
//! holds a service-label lock through retirement, replacement, and matching process
//! readiness. Failed installation preserves the desired registration for forward
//! recovery; it never rolls back or stops an unretired replacement. Its `control`
//! module owns private retirement exchange files, bounded health parsing, and
//! launchctl effects. Its `generation`
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
//! bundled windows -> commands authorization -> Gateway startup / endpoint / credential access
//!                    -> Claude configuration directory -> one re-registration
//! desktop window  -> commands authorization -> endpoint / credential access, once ready
//! Gateway -> native manager -> staging -> verified immutable runtime
//!                    -> control -> launchd / existing gateway
//!                    -> systemd -> typed D-Bus jobs / pidfds
//!                    -> pruning -> superseded runtime versions
//!         -> reconciliation audit -> private atomic intent/outcome records
//!         -> LoginShell -------> the account's login shell
//! ```
//! Arrows mean calls; commands translate the application-owned startup contract
//! and verify the caller's window for native gateway access: the bundled
//! surfaces may bring the gateway up, the desktop window only reads it once it
//! is ready (`GatewayReader`). Each native
//! manager adapter owns its background process lifetime.
mod commands;
#[cfg(any(target_os = "linux", all(test, unix)))]
mod linux;
mod login_shell;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod reconciliation_audit;
mod reconciliation_ids;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod retirement;
mod selection;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unsupported;
pub use commands::{
    __cmd__gateway_startup, __cmd__retry_gateway_startup,
    __cmd__set_claude_configuration_directory, __tauri_command_name_gateway_startup,
    __tauri_command_name_retry_gateway_startup,
    __tauri_command_name_set_claude_configuration_directory, gateway_startup,
    retry_gateway_startup, set_claude_configuration_directory, startup_events,
};
pub(crate) use commands::{GatewayReader, GatewayUnread};
pub use selection::{
    current, login_shell_path, platform_context, reconciliation_audit, reconciliation_ids,
};
