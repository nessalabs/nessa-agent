//! Immutable, validated values of the gateway context.
mod reconciliation_evidence;
#[cfg(any(target_os = "macos", test))]
pub use reconciliation_evidence::ReconciliationRuntimeIdentity;
pub use reconciliation_evidence::{
    BundledSurface, PendingReconciliation, ReconciliationAttemptRecord, ReconciliationCause,
    ReconciliationCleanupDecision, ReconciliationCorrelation, ReconciliationEffectTimingRecord,
    ReconciliationEvidence, ReconciliationHistory, ReconciliationHistoryFact,
    ReconciliationIncarnation, ReconciliationInitiator, ReconciliationIntentDeliveryRecord,
    ReconciliationIntentRecord, ReconciliationOutcomeDisposition, ReconciliationOutcomeRecord,
    ReconciliationPhysicalRecord, ReconciliationRejectedReport, ReconciliationRequestRecord,
    ReconciliationTarget, ReconciliationValidationFacts,
};
mod search_path;
pub use search_path::{SearchPath, SearchPathError};
