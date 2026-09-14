//! A local invocation stop with its lifecycle cause and responsible initiator.
#![deny(missing_docs)]

use super::{SchedulingCause, SchedulingInitiator};
use std::{error::Error, fmt};

/// Immutable cause and initiator of a local invocation stop.
/// The containing invocation supplies its identity; the application retains the
/// verified caller identity for caller-initiated cancellation. This value does
/// not claim that the provider received or cancelled any work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvocationCancellation {
    cause: SchedulingCause,
    initiator: SchedulingInitiator,
}

/// The cause and initiator do not describe a supported local cancellation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvocationCancellationError;
impl fmt::Display for InvocationCancellationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid invocation cancellation cause or initiator")
    }
}
impl Error for InvocationCancellationError {}

impl InvocationCancellation {
    /// Validate the lifecycle `cause` and responsible `initiator`.
    /// Explicit session close requires a caller; a stopped runner is automatic.
    /// Other combinations return an error. No effects are performed.
    pub fn new(
        cause: SchedulingCause,
        initiator: SchedulingInitiator,
    ) -> Result<Self, InvocationCancellationError> {
        if !matches!(
            (cause, initiator),
            (SchedulingCause::SessionClosed, SchedulingInitiator::Caller)
                | (
                    SchedulingCause::RunnerStopped,
                    SchedulingInitiator::Automatic
                )
        ) {
            return Err(InvocationCancellationError);
        }
        Ok(Self { cause, initiator })
    }

    /// Why the invocation owner stopped this input.
    pub fn cause(&self) -> SchedulingCause {
        self.cause
    }

    /// Whether cancellation belongs to an explicit caller or the runtime.
    pub fn initiator(&self) -> SchedulingInitiator {
        self.initiator
    }
}
