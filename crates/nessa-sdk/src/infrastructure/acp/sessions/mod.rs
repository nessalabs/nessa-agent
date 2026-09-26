//! Owns process attachment, configuration, close, and automatic resume.
//!
//! ```text
//! profile + AcpConfig -> binding -> execution worker
//!                         |
//!                    application ProviderSession
//!                         |
//!                    cleanup -> retained process or pre-start directory
//! ```
//! Arrows show construction. The stable client replaces a cleaned-up connection;
//! only a successful provider resume restores the same context. Unreported idle
//! failures are surfaced before replacement; old readers drain their correlated
//! evidence without replaying an already-reported failure into a new invocation.
//! Failed startup and live teardown retain uncertain resource ownership for
//! cleanup retry; confirmation cannot erase a separately failed audit delivery.
//! Losing the final recovery handle transfers the resource to a physical cleanup
//! supervisor, including when an opening caller disappears. `deletion` opens a
//! connection of its own, never resuming the session it asks the agent to
//! delete.
pub(crate) mod binding;
pub(crate) mod cleanup;
mod clock;
mod config;
pub(crate) mod configuration;
pub(crate) mod deletion;
pub(crate) mod identity;
pub use clock::{AcpClock, BudgetExpiry, RuntimeClock};
pub use config::{AcpConfig, StdioMcpServer};
