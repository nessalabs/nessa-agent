//! Immutable, validated values of the gateway context.
mod lifecycle_journal;
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
pub use lifecycle_journal::LifecycleJournalError;
pub use lifecycle_journal::{
    AuditDeliveryReceipt, LifecycleCommandResult, LifecycleEffect, LifecycleEffectPredicate,
    LifecycleFailedPhase, LifecycleObservation, LifecycleObservationSource,
    LifecyclePhysicalOutcome, LifecyclePlanStep, LifecycleRecordKind,
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use lifecycle_journal::{
    LifecycleHistory, LifecyclePendingStep, LifecycleRecord, LifecycleRecordPayload, ServiceManager,
};
mod reconciliation_evidence;
#[cfg(test)]
pub use reconciliation_evidence::ReconciliationRuntimeIdentity;
pub use reconciliation_evidence::{
    BundledSurface, PendingReconciliation, ReconciliationAttemptRecord, ReconciliationCause,
    ReconciliationCleanupDecision, ReconciliationCorrelation, ReconciliationEffectTimingRecord,
    ReconciliationEvidence, ReconciliationHistory, ReconciliationHistoryFact,
    ReconciliationIncarnation, ReconciliationInitiator, ReconciliationIntentDeliveryRecord,
    ReconciliationIntentRecord, ReconciliationOutcomeDisposition, ReconciliationOutcomeRecord,
    ReconciliationPhysicalRecord, ReconciliationRejectedReport, ReconciliationRequestRecord,
    ReconciliationTarget,
};
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod retirement_refusal;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub use retirement_refusal::RetirementRefusal;
mod search_path;
pub use search_path::{SearchPath, SearchPathError};
mod service_configuration;
pub use service_configuration::ServiceConfiguration;
mod systemd;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use systemd::SystemdInvocationId;
pub use systemd::{
    SystemdEvidenceError, SystemdJobAttempt, SystemdJobMode, SystemdJobOperation,
    SystemdManagerIdentity, SystemdRuntimeObservation, SystemdUnitName, SystemdUnitState,
};
#[cfg(target_os = "linux")]
pub use systemd::{SystemdJobConclusion, SystemdJobTerminal};
