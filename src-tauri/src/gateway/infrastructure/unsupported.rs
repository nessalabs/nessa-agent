use crate::gateway::application::{
    GatewayError, GatewayHost, GatewayReconciliationAttempt, GatewayReconciliationAudit,
    GatewayReconciliationIntent, GatewayReconciliationJournalSession, GatewayReconciliationOutcome,
    GatewayReconciliationProgress, GatewayReconciliationRequest, GatewayStopSession,
    ReconciledGateway,
};
use crate::gateway::domain::value_objects::{
    AuditDeliveryReceipt, LifecycleCommandResult, LifecycleObservation, LifecycleObservationSource,
    LifecyclePhysicalOutcome, LifecyclePlanStep, LifecycleRecordKind,
    ReconciliationCleanupDecision, ReconciliationCorrelation, ReconciliationIncarnation,
    ReconciliationTarget, SearchPath,
};
use std::{path::Path, sync::Arc};
pub(super) struct Unsupported;
pub(super) struct UnsupportedAudit;

impl GatewayReconciliationAudit for UnsupportedAudit {
    fn open(
        self: Arc<Self>,
        attempt: &GatewayReconciliationAttempt,
        _: Option<std::time::Instant>,
    ) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError> {
        Ok(Arc::new(UnsupportedJournalSession(
            attempt.correlation().clone(),
        )))
    }
}

struct UnsupportedJournalSession(ReconciliationCorrelation);

impl GatewayReconciliationJournalSession for UnsupportedJournalSession {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        Ok(())
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        Ok(())
    }

    fn joined(&self, _: &GatewayReconciliationRequest) -> Result<(), GatewayError> {
        Ok(())
    }

    fn effect_plan(
        &self,
        _: &str,
        _: Option<&ReconciliationIncarnation>,
        _: &ReconciliationTarget,
        _: &LifecyclePlanStep,
        _: &[LifecyclePlanStep],
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        Ok(AuditDeliveryReceipt::new(
            self.0.clone(),
            1,
            LifecycleRecordKind::EffectPlan,
        ))
    }

    fn effect_completion(
        &self,
        _: &str,
        _: &str,
        _: &LifecycleCommandResult,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        Ok(AuditDeliveryReceipt::new(
            self.0.clone(),
            2,
            LifecycleRecordKind::EffectCompletion,
        ))
    }

    fn observation(
        &self,
        _: &LifecycleObservationSource,
        _: &LifecycleObservation,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        Ok(AuditDeliveryReceipt::new(
            self.0.clone(),
            3,
            LifecycleRecordKind::Observation,
        ))
    }

    fn physical_outcome(
        &self,
        _: &LifecyclePhysicalOutcome,
        _: Option<&LifecycleObservation>,
        _: ReconciliationCleanupDecision,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        Ok(AuditDeliveryReceipt::new(
            self.0.clone(),
            4,
            LifecycleRecordKind::Outcome,
        ))
    }
}
impl GatewayHost for Unsupported {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
        _: &GatewayReconciliationAttempt,
        _: &dyn GatewayReconciliationProgress,
    ) -> Result<ReconciledGateway, GatewayError> {
        Err(GatewayError::Registration(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
    fn stop_agents(
        &self,
        _: &GatewayStopSession,
        _: &dyn GatewayReconciliationJournalSession,
        _: &AuditDeliveryReceipt,
    ) -> Result<LifecycleObservation, GatewayError> {
        Err(GatewayError::Stop(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
}
