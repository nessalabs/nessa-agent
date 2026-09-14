//! Delivery intent stays immutable for the lifetime of one logical input.

/// Delivery intent of a submitted input; changing it creates a different submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionMode {
    /// Execute immediately when the session is available.
    Immediate,
    /// Wait in the sequential queue.
    Queued,
    /// Run with priority at an invocation boundary.
    BoundarySteering,
    /// Request steering: inject into an active invocation when supported, otherwise
    /// queue with priority if no input was consumed. With no active invocation,
    /// queue directly without an injection attempt or target.
    /// This intent stays unchanged on retry, even when delivery uses the queue.
    Steering,
}
