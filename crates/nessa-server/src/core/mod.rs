//! Process bootstrap and cross-cutting infrastructure.
//!
//! Owns everything that applies to the whole binary before any feature runs:
//! tracing setup, the gateway log's size bound, fatal error reporting, the
//! tokio runtime wrapper in `bootstrap`, and the trusted-origin predicate every
//! context shares in `trusted_origin`.
//!
//! How a run ends is three decisions taken together: what to say (`error`),
//! which number says it (`exit_code`), and whether launchd should start the
//! process again (`restart`). A failure that starting again cannot fix exits
//! zero — the only thing launchd reads as "stop" — and leaves its reason in
//! `startup_failure` for the desktop host, because a zero status carries none.
//!
//! ```text
//! main ──► core::run ──► logging::init
//!                    ├──► log_file::bound (gateway.log, size)
//!                    └──► composition::CompositionRoot::serve
//!                              │
//!                         RunError ──► error::report
//!                                        ├─ log line + exit status
//!                                        └─ startup_failure::record (gave up)
//! ```

mod bootstrap;
mod error;
mod exit_code;
#[cfg(unix)]
mod log_file;
pub mod logging;
mod restart;
pub mod startup_failure;
pub mod trusted_origin;

pub use bootstrap::run;
pub use error::RunError;
