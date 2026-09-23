use crate::gateway::application::{
    GatewayError, GatewayHost, GatewayReconciliationAttempt, GatewayReconciliationAudit,
    GatewayReconciliationIntent, GatewayReconciliationOutcome, GatewayReconciliationProgress,
    GatewayReconciliationRequest, ReconciledGateway,
};
use crate::gateway::domain::value_objects::SearchPath;
use std::path::Path;
pub(super) struct Unsupported;
pub(super) struct UnsupportedAudit;

impl GatewayReconciliationAudit for UnsupportedAudit {
    fn intent(&self, _: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        Ok(())
    }

    fn outcome(&self, _: &GatewayReconciliationOutcome) -> Result<(), GatewayError> {
        Ok(())
    }

    fn joined(
        &self,
        _: &GatewayReconciliationAttempt,
        _: &GatewayReconciliationRequest,
    ) -> Result<(), GatewayError> {
        Ok(())
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
    fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
        Err(GatewayError::Stop(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
}
