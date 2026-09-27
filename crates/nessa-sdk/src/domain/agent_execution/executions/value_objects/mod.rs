//! Immutable execution identity, output, and scheduling descriptions without effects.
//!
//! ```text
//! application event --> ExecutionId + MessageChunk / ExecutionOutcome
//! MessageChunk --> MessageKind + optional MessageId + immutable text
//! scheduling evidence --> SchedulingTransition --> validated stage/cause/initiator
//! undispatched immediate close --> InvocationCancellation --> cause/initiator
//! ordered transitions --> validate_history --> continuous stages + stable kind/target
//! ```
//!
//! The arrow means the application combines these values to correlate output.
//! The output values themselves do not depend on an execution identity.
//! QueueMutation describes membership facts; the InvocationQueue aggregate applies
//! them. QueueOrderChange retains a validated exact priority-preserving permutation.
//! Scheduling transitions validate legal edges and retain target/cause meaning.
//! Their history validator owns continuity across successive values;
//! the application supplies caller attribution and performs storage effects.
mod cancellation;
mod identity;
mod message;
mod scheduling;
mod transition;
pub use cancellation::{InvocationCancellation, InvocationCancellationError};
pub use identity::ExecutionId;
pub use message::{ExecutionOutcome, MessageChunk, MessageId, MessageKind};

pub use scheduling::{InvocationKind, InvocationStage, SchedulingCause};

pub use transition::{SchedulingInitiator, SchedulingTransition, SchedulingTransitionError};

mod submission;
pub use submission::SubmissionMode;

mod queue_order;
pub use queue_order::{QueueOrderChange, QueueOrderError};

mod queue_history;
pub use queue_history::{QueueMutation, QueueRemovalCause};
