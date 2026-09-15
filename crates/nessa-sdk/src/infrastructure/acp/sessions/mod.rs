//! Owns process attachment, configuration, close, and automatic resume.
//!
//! ```text
//! profile + AcpConfig -> binding -> execution worker
//!                         |
//!                    application ProviderSession
//!                         |
//!                    cleanup -> retained ProcessScope
//! ```
//! Arrows show construction. The stable client replaces a cleaned-up connection;
//! only a successful provider resume restores the same context. Unreported idle
//! failures are surfaced before replacement; old readers drain their correlated
//! evidence without replaying an already-reported failure into a new invocation.
//! Failed startup and live teardown retain uncertain process ownership for cleanup
//! retry; confirmation cannot erase a separately failed audit delivery.
//! Losing the final recovery handle transfers an uncertain scope to a physical
//! cleanup supervisor, including when an opening caller disappears.
pub(crate) mod binding;
pub(crate) mod cleanup;
mod config;
pub use config::AcpConfig;
