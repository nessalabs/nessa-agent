//! Process bootstrap and cross-cutting infrastructure.
//!
//! Owns everything that applies to the whole binary before any feature runs:
//! tracing setup, fatal error reporting, the tokio runtime wrapper in `bootstrap`,
//! and the trusted-origin predicate every context shares in `trusted_origin`.
//!
//! ```text
//! main ──► core::run ──► logging::init
//!                    └──► composition::CompositionRoot::serve
//!                              │
//!                         RunError ──► Termination (exit code + log)
//! ```

mod bootstrap;
mod error;
pub mod logging;
pub mod trusted_origin;

pub use bootstrap::run;
pub use error::RunError;
