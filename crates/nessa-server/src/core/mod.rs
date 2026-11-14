//! Process bootstrap and cross-cutting infrastructure.
//!
//! Owns everything that applies to the whole binary before any feature runs:
//! tracing setup, Linux process-local dump/inspection protection, the gateway
//! log's size bound, fatal error reporting, the
//! tokio runtime wrapper in `bootstrap`, and the trusted-origin predicate every
//! context shares in `trusted_origin`. `local_timings` is the duration files kept in the
//! log directory; `nessa doctor` reads them and nothing sends them.
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
pub(crate) use nessa_conversation::limit_log;
pub(crate) mod local_timings;
#[cfg(unix)]
mod log_file;
pub mod logging;
#[cfg(unix)]
mod open_files;
#[cfg(target_os = "linux")]
mod process_memory;
mod restart;
#[cfg(unix)]
pub(crate) mod self_restart;
mod shutdown;
mod startup_failure;
pub mod trusted_origin;

pub use bootstrap::run;
pub use error::{
    Dataset, DatasetRefusal, MissingReceiverJournal, NativeFailure, RunError, ServeAndShutdown,
};
pub use launch::Launch;
pub use shutdown::{
    NativeShutdownFailure, Outcome, ReaderDrain, ServersOutcome, ShutdownFailure, ShutdownReport,
    ShutdownStage,
};
