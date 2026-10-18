//! The standalone verifier's process arguments; no profile lookup or I/O.
//!
//! `arguments` owns the command grammar. Composition consumes its typed result.

mod arguments;

pub use arguments::{parse_verification, VerificationCommand, VerificationUsageError, VERIFY_HELP};
