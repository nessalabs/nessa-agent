//! Agent-facing calls into the single attachment lifecycle owner.
use super::lifecycle::{CloseAttempt, WorkPermit};
use super::{Agent, AgentError};
use crate::application::agent_execution::providers::{
    ProviderOperationFailure, ProviderSessionState, SessionCloseRequest,
};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::permissions::PermissionRequest;
use std::{future::Future, sync::Arc};

pub(super) type AcceptedControl = Arc<WorkPermit>;
impl Agent {
    pub(super) fn start_shutdown(&self, request: SessionCloseRequest) -> CloseAttempt {
        self.inner.lifecycle.start_stop(request)
    }
    pub(super) fn accept_control(&self) -> Result<AcceptedControl, AgentError> {
        self.inner.lifecycle.accept_control().map(Arc::new)
    }
    pub(super) async fn run_control<T>(
        &self,
        admission: AcceptedControl,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
    ) -> Result<T, AgentError> {
        self.inner
            .lifecycle
            .run_control(&admission, operation)
            .await
    }
    pub(super) async fn run_control_observed<T>(
        &self,
        admission: AcceptedControl,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
    ) -> Result<T, ProviderOperationFailure> {
        self.inner
            .lifecycle
            .run_control_observed(&admission, operation)
            .await
    }
    // A received provider receipt is no longer an interruptible provider wait.
    // Keep its work permit through local validation so close cannot erase the
    // acknowledgement or reopen the generation before validation finishes.
    pub(super) async fn validate_permission_receipt(
        &self,
        admission: &WorkPermit,
        request: &PermissionRequest,
        input: &ToolReviewInput,
    ) -> Result<(), AgentError> {
        let result = self
            .inner
            .manager
            .validate_permission_evidence(request, input)
            .await;
        if result.is_err() {
            self.inner
                .lifecycle
                .record_control_state(admission, &ProviderSessionState::CleanupRequired);
        }
        result
    }
    pub(super) fn accept_preparation(&self) -> Result<AcceptedControl, AgentError> {
        self.inner.lifecycle.accept_work().map(Arc::new)
    }
    pub(super) async fn run_preparation<T>(
        &self,
        admission: AcceptedControl,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
    ) -> Result<T, AgentError> {
        self.inner
            .lifecycle
            .run_preparation(&admission, operation)
            .await
    }
    pub(super) fn stop_control_admission(&self) {
        self.inner
            .lifecycle
            .block(self.inner.lifecycle.work_generation());
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/agents/coordination.rs"]
mod tests;
