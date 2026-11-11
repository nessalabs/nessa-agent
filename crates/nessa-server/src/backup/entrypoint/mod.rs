//! Standalone backup commands' process arguments; no profile lookup or I/O.
//!
//! `arguments` owns each command grammar. Composition consumes the typed result.

mod arguments;
mod restore_arguments;

pub use arguments::{parse_verification, VerificationCommand, VerificationUsageError, VERIFY_HELP};
pub use restore_arguments::{parse_restore, RestoreCommand, RestoreUsageError, RESTORE_HELP};
