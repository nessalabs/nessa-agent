//! Immutable invocation priority, stages, and causal lifecycle descriptions.
#![deny(missing_docs)]

/// Determines which pending invocation is dispatched first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationKind {
    /// Ordinary follow-up work, dispatched after pending steering work.
    Queued,
    /// Work prioritized at the next dispatch boundary; it does not interrupt work.
    Steering,
}

/// Local scheduling state recorded independently of confirmed provider effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationStage {
    /// Admitted and awaiting dispatch.
    Queued,
    /// Selected for an execution attempt, without confirming provider execution.
    Running,
    /// Steering input was accepted by the provider into a separate active execution.
    Injected,
    /// The execution attempt ended; its cause distinguishes a known outcome from failure.
    Settled,
    /// Locally cancelled; this does not prove an external effect was rolled back.
    Cancelled,
}

/// Lifecycle reason for a local invocation scheduling transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulingCause {
    /// A caller submitted the invocation.
    Submitted,
    /// The runner selected the invocation for execution.
    Dispatched,
    /// Preparation or dispatch did not confirm successful admission. An ambiguous
    /// native delivery failure still must not be replayed.
    DispatchFailed,
    /// The provider confirmed accepting steering input into the active execution.
    SteeringInjected,
    /// Execution reported a terminal outcome.
    ExecutionSettled,
    /// The execution attempt failed without retaining an exact terminal outcome.
    /// The invocation keeps its local failure separately from provider cleanup.
    ExecutionFailed,
    /// A verified caller explicitly closed the owning agent session.
    SessionClosed,
    /// The local queue runner stopped after a failed invocation or dispatch, so
    /// remaining inputs cannot safely run. The failed invocation retains its
    /// detailed error; this cause does not describe provider-session closure.
    /// Provider closure evidence carries its own deadline/loss/cleanup reason.
    RunnerStopped,
    /// A verified caller removed pending input before dispatch.
    Withdrawn,
    /// The host stopped the agent for its own shutdown, not to end this work.
    /// The input is owed: the next start resumes it with a successor turn.
    Suspended,
    /// Found unfinished when the agent was opened again after its process
    /// ended — no result, and nothing left running it. The input is owed in
    /// the same way as a suspended one.
    Interrupted,
    /// The host decided not to resume an owed input: the share grant that
    /// started it no longer admits it, or it was resumed as many times as a
    /// chain may be. Final, like a person's stop. The one edge that may
    /// follow a cancellation, and only one that left the input owed.
    NotResumed,
}
impl SchedulingCause {
    /// Whether this cancellation leaves the input owed, to be resumed by a
    /// successor turn, rather than final: the host's own shutdown or restart
    /// ended it, not a person, a failure or the model.
    pub fn leaves_work_owed(self) -> bool {
        matches!(self, Self::Suspended | Self::Interrupted)
    }
}
