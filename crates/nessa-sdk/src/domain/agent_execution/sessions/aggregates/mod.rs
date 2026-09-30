//! The live session aggregate protects lifecycle and review identity invariants.
//!
//! ```text
//! application --> ExecutionSession --> ActiveExecution --> tools + permissions
//! ```
//!
//! Arrows mean calls into the aggregate and the state it owns. Closing returns
//! SessionClosureResult with session-transition evidence and cancelled reviews.
mod execution_session;
pub(crate) use execution_session::HistoricalToolUndo;
pub use execution_session::{ExecutionSession, SessionClosureResult};
