//! A background command's identity, bounds and independently settled outcome.
//! The application owns execution and delivery; the domain owns valid inputs.

mod lifecycle;
mod values;
pub use lifecycle::{TaskDelivery, TaskInitiator, TaskLifecycle, TaskStop, TaskTransitionError};

pub use values::{BackgroundCommand, BackgroundTaskId, TaskCause, TaskInputError, TaskOutcome};
