//! Pure reasons for beginning a provider attachment.

use super::ExecutionSessionId;
use std::{error::Error, fmt};

/// Why the session lifecycle authorized one provider attachment attempt.
///
/// The lifecycle derives initial versus restored context from durable session
/// evidence. A caller cannot label a fresh session as a recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentCause {
    /// A verified caller requested the first provider context for this session.
    Initial,
    /// A verified caller requested restoration of the recorded provider context.
    Reopen,
    /// The lifecycle's confirmed automatic-recovery state authorized restoration.
    AutomaticRecovery,
}

/// Durable provider-context fact owned by the session boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderContext {
    /// No provider context has ever been published for this session.
    Absent,
    /// Exact provider context identity that later attachment must restore.
    Recorded(ExecutionSessionId),
}
impl ProviderContext {
    /// Recorded context identity, when attachment has published one.
    pub fn recorded(&self) -> Option<&ExecutionSessionId> {
        match self {
            Self::Absent => None,
            Self::Recorded(id) => Some(id),
        }
    }

    /// Whether retained provider-side evidence is valid for this context state.
    pub fn permits_provider_evidence(&self) -> bool {
        matches!(self, Self::Recorded(_))
    }

    /// Validate whether a restored checkpoint's mapped facts require a context.
    pub fn validate_evidence(
        &self,
        has_provider_observations: bool,
        has_provider_report: bool,
        has_dispatched_stage: bool,
        has_native_correlation: bool,
        has_selected_queue_entry: bool,
    ) -> Result<(), ProviderContextEvidenceError> {
        if !self.permits_provider_evidence()
            && (has_provider_observations
                || has_provider_report
                || has_dispatched_stage
                || has_native_correlation
                || has_selected_queue_entry)
        {
            return Err(ProviderContextEvidenceError);
        }
        Ok(())
    }
}

/// A checkpoint claims provider-side activity without a published context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderContextEvidenceError;
impl fmt::Display for ProviderContextEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("provider evidence requires a recorded provider context")
    }
}
impl Error for ProviderContextEvidenceError {}
