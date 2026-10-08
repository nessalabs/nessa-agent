//! What the choice needs from outside the process: one logind, one audit sink.

use crate::linger::domain::{
    LingerCall, LingerCorrelation, LingerDecline, LingerIntent, LingerOutcome, LingerSnapshot,
    LoginUserId,
};

/// logind's linger bit for this process's user, and the one call that turns it on.
///
/// There is no disable. Turning linger off is `loginctl disable-linger`, outside
/// this process.
pub(crate) trait LogindLinger: Send + Sync {
    fn read(&self) -> LingerSnapshot;

    /// `SetUserLinger(user, enable: true, interactive: true)` for that user.
    fn enable(&self, user: LoginUserId) -> LingerCall;
}

/// Intent before a call, outcome after the confirming read, decline when the
/// person chose not to enable.
pub(crate) trait LingerAudit: Send + Sync {
    fn record_intent(&self, intent: &LingerIntent)
        -> Result<LingerCorrelation, LingerAuditFailure>;

    fn record_outcome(
        &self,
        correlation: &LingerCorrelation,
        outcome: &LingerOutcome,
    ) -> Result<(), LingerAuditFailure>;

    fn record_decline(&self, decline: &LingerDecline) -> Result<(), LingerAuditFailure>;
}

/// The sink could not store the record. The choice stops before a call when
/// this is the intent; a finished call still reports the confirming read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LingerAuditFailure;
