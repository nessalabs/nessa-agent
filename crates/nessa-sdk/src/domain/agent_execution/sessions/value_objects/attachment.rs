//! Attachment causes and retained native-context admission evidence.

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
    Recorded(ProviderContextRecord),
}
impl ProviderContext {
    /// Publish a native context and the retained invocation prefix preceding it.
    ///
    /// `admitted_before` counts invocations, not provider observations. Capture
    /// it before awaiting fresh native opening; later admission remains counted
    /// even when it settles before context publication. Zero is conservative:
    /// any retained invocation then requires native restoration.
    /// Storage admission validates this boundary against the retained history;
    /// constructing the value alone does not establish a valid checkpoint.
    pub fn recorded_after(id: ExecutionSessionId, admitted_before: usize) -> Self {
        Self::Recorded(ProviderContextRecord {
            id,
            admitted_before,
        })
    }

    /// Count of retained invocations excluded from this context's admission.
    /// `None` means no native context has been published.
    pub fn admission_watermark(&self) -> Option<usize> {
        match self {
            Self::Absent => None,
            Self::Recorded(record) => Some(record.admitted_before),
        }
    }

    /// Whether retained admission requires preserving this native context.
    ///
    /// `invocation_count` is the validated retained history length. An invalid
    /// count smaller than the watermark is conservatively treated as used;
    /// [`Self::validate_admission`] reports that contradiction separately.
    pub fn has_admitted_work(&self, invocation_count: usize) -> bool {
        self.admission_watermark()
            .is_some_and(|watermark| invocation_count != watermark)
    }

    /// Validate the context boundary against retained invocation evidence.
    ///
    /// `terminal_prefix` is the number of consecutive terminal invocations at
    /// the beginning of a history containing `invocation_count` invocations.
    /// The excluded prefix must be terminal, so queued work admitted while a
    /// native context opens cannot disappear behind its watermark.
    ///
    /// # Errors
    /// Returns [`ProviderContextAdmissionError::InvalidWatermark`] if either
    /// count contradicts the retained boundary. No state is changed.
    pub fn validate_admission(
        &self,
        invocation_count: usize,
        terminal_prefix: usize,
    ) -> Result<(), ProviderContextAdmissionError> {
        if terminal_prefix > invocation_count
            || self
                .admission_watermark()
                .is_some_and(|watermark| watermark > terminal_prefix)
        {
            return Err(ProviderContextAdmissionError::InvalidWatermark);
        }
        Ok(())
    }

    /// Validate publication of a replacement context at the current boundary.
    ///
    /// `previous` is the context being replaced. `namespace_changed` records a
    /// provider identity or handoff change in this same atomic decision. A
    /// native ID reused without that evidence cannot advance its watermark.
    /// A fresh context preserves the terminal-prefix boundary captured before
    /// awaiting its opening. That boundary may be below the terminal prefix at
    /// publication when later admitted work has already settled. Retained evidence
    /// validates the boundary against the current prefix; it does not reconstruct
    /// opening time. An existing boundary is preserved on resume. A context with
    /// admitted work cannot be cleared because observations are absent.
    ///
    /// # Errors
    /// Returns [`ProviderContextAdmissionError`] for malformed retained counts,
    /// a boundary skipping pending work, or an unauthorized same-context reset.
    pub fn validate_transition(
        &self,
        previous: &Self,
        invocation_count: usize,
        terminal_prefix: usize,
        namespace_changed: bool,
    ) -> Result<(), ProviderContextAdmissionError> {
        self.validate_admission(invocation_count, terminal_prefix)?;
        if self.admission_watermark().is_none() {
            return if previous.has_admitted_work(invocation_count) {
                Err(ProviderContextAdmissionError::InvalidTransition)
            } else {
                Ok(())
            };
        }
        if self.recorded() == previous.recorded()
            && !namespace_changed
            && self.admission_watermark() != previous.admission_watermark()
        {
            return Err(ProviderContextAdmissionError::InvalidTransition);
        }
        Ok(())
    }

    /// Recorded context identity, when attachment has published one.
    pub fn recorded(&self) -> Option<&ExecutionSessionId> {
        match self {
            Self::Absent => None,
            Self::Recorded(record) => Some(&record.id),
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

/// Native context identity and the admission boundary published with it.
///
/// Its fields are immutable; construction and borrowed inspection belong to
/// [`ProviderContext`]. Retained storage must preserve both facts together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderContextRecord {
    id: ExecutionSessionId,
    admitted_before: usize,
}

/// Retained history contradicts a context's admission boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderContextAdmissionError {
    /// The watermark skips pending work or lies outside the retained history.
    InvalidWatermark,
    /// A replacement resets evidence without the matching publication boundary.
    InvalidTransition,
}
impl fmt::Display for ProviderContextAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidWatermark => {
                "provider context watermark exceeds its terminal invocation prefix"
            }
            Self::InvalidTransition => {
                "provider context watermark does not match its publication boundary"
            }
        })
    }
}
impl Error for ProviderContextAdmissionError {}
