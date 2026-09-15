#![deny(missing_docs)]

/// Provider operations available to an Agent, separate from model modalities and
/// token limits. This is a point-in-time support snapshot, not an admission permit:
/// an operation can still fail because the session is busy, closed, or unavailable.
///
/// ACP reports the latest successfully negotiated connection. Restoration clears
/// the snapshot while reconnecting and publishes new support after validation.
/// Closing retains the last negotiation; it does not promise restoration success.
/// Custom backends default to no advertised support and opt in explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationCapabilities {
    /// The provider can inject input into an identified active invocation.
    /// This does not describe SDK-owned queueing or next-invocation steering.
    pub native_steering: bool,
    /// The provider supports restoring the same context after connection close.
    /// Saved history can still be missing or unavailable when restoration is tried.
    pub session_resume: bool,
}
