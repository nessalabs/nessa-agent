//! Execution identities, output values, and pending invocation scheduling rules.
//! Sessions, tools, and reviews use identities to correlate their state; the
//! application owns running work and effects around the pending queue.
//!
//! ```text
//! sessions + tools + permissions --> execution values
//! application runner --> invocation queue --> execution identity
//! ```
//!
//! Arrows indicate dependencies on domain values and scheduling decisions.
pub mod aggregates;
pub mod entities;
mod error;
pub mod value_objects;
pub use aggregates::InvocationQueue;
pub use entities::{InvocationHistory, InvocationHistoryError, InvocationObservation};
pub use error::SchedulingError;
pub use value_objects::{
    ExecutionId, ExecutionOutcome, InvocationCancellation, InvocationCancellationError,
    InvocationKind, InvocationStage, MessageChunk, MessageKind, SchedulingCause,
    SchedulingInitiator, SchedulingTransition, SchedulingTransitionError, SubmissionMode,
};
