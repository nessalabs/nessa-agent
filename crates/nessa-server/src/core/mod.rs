//! Process bootstrap and cross-cutting infrastructure.
//!
//! Owns everything that applies to the whole binary before any feature runs:
//! tracing setup, the gateway log's size bound, fatal error reporting, the
//! tokio runtime wrapper in `bootstrap`, and the trusted-origin predicate every
//! context shares in `trusted_origin`.
//!
//! How a run ends is decided in `ending`, out of four facts: what went wrong
//! (`error`), which number says it (`exit_code`), whether launchd should start
//! the process again (`restart`), and whether this process is the service
//! launchd supervises at all (`launch`). Only that service may exit zero — the
//! one thing launchd reads as "stop" — and only after `startup_failure` has
//! durably published the reason a zero status cannot carry, because that record
//! is the only route back for a service launchd will not restart. Being asked
//! to stop is not that, so a supervised gateway that served and was signalled
//! exits non-zero and comes back; `launchctl bootout` is how it is stopped.
//!
//! ```text
//! main ──► core::run ──► logging::init
//!                    ├──► log_file::bound (gateway.log, size)
//!                    ├──► Launch::from_system (managed by launchd, or not)
//!                    └──► composition::CompositionRoot::serve
//!                              │
//!                     Result<(), RunError> ──► ending::report
//!                                        ├─ startup_failure::record (gave up)
//!                                        └─ exit status + log line
//! ```

mod bootstrap;
mod ending;
mod error;
mod exit_code;
pub mod launch;
#[cfg(unix)]
mod log_file;
pub mod logging;
mod restart;
mod startup_failure;
pub mod trusted_origin;

pub use bootstrap::run;
pub use error::{Dataset, DatasetRefusal, RunError};
pub use launch::Launch;
