use crate::{
    conversation::application::{ConversationError, ConversationService},
    desktop_runtime::domain::{
        RetirementFence, RetirementRefusal, RetirementRequest, RunningRuntime,
    },
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use std::{future::Future, pin::Pin, time::Duration};

pub(crate) struct RetirementRecord {
    pub request: RetirementRequest,
    pub running: RunningRuntime,
    pub cleanup_error: Option<String>,
    pub retirement_cause: Option<ActionContext>,
}
#[derive(Clone, Debug)]
pub(crate) struct RetirementResult {
    pub request: RetirementRequest,
    pub running: RunningRuntime,
    pub retired: bool,
    pub retirement_cause: Option<ActionContext>,
    // This is the complete cleanup operation diagnostic, not proof of physical state.
    pub cleanup_error: Option<String>,
    pub audit_error: Option<String>,
    /// Why not, whenever `retired` is false; what the desktop host acts on.
    pub refusal: Option<RetirementRefusal>,
}
impl RetirementResult {
    #[cfg(test)]
    pub(crate) fn retirement_request_id(&self) -> Option<&str> {
        self.retirement_cause
            .as_ref()
            .map(ActionContext::request_id)
    }
}
/// Whether this gateway's own conversation data is still where it was opened.
/// Asked only when a retirement did not happen, to say why (ADR 221).
pub(crate) trait ConversationData: Send + Sync {
    fn missing(&self) -> bool;
}
/// Work this gateway runs outside its conversations, such as the startup
/// warm-ups, that may hold an agent process of its own. Asked only when a
/// retirement did not happen, to say why (ADR 221).
pub(crate) trait BackgroundWork: Send + Sync {
    fn may_hold_resources(&self) -> bool;
}
pub(crate) trait RetirementAudit: Send + Sync {
    fn record(
        &self,
        record: RetirementRecord,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}
pub(crate) async fn retire(
    request: RetirementRequest,
    running: RunningRuntime,
    conversations: Option<&ConversationService>,
    data: &dyn ConversationData,
    background: &dyn BackgroundWork,
    audit: &dyn RetirementAudit,
) -> RetirementResult {
    // Whether the stops failed without leaving anything running: the one
    // failure a refusal may call `data_missing` (ADR 221).
    let mut nothing_left_running = false;
    let mut cleanup_error = if !running.accepts(&request) {
        Some(
            "Retirement request identifies a different running instance or service generation"
                .into(),
        )
    } else {
        match conversations {
            Some(service) => match service.retire("gateway_upgrade", request.id()).await {
                Ok(()) => None,
                Err(error) => {
                    // An admission that could not drain may still be running
                    // work; only a stop failure is asked about what remains.
                    nothing_left_running = matches!(error, ConversationError::Retirement(_))
                        && !service.owns_unreleased_resources().await
                        && !background.may_hold_resources();
                    Some(error.to_string())
                }
            },
            None => None,
        }
    };
    if cleanup_error.is_none()
        && conversations
            .and_then(ConversationService::retirement_cause)
            .is_some_and(|cause| cause.surface_id() != "gateway_upgrade")
    {
        cleanup_error = Some("runtime admission was closed by a different lifecycle cause".into());
        nothing_left_running = false;
    }
    let retirement_cause = if running.accepts(&request) {
        Some(
            conversations
                .and_then(ConversationService::retirement_cause)
                .unwrap_or_else(|| {
                    ActionContext::new("gateway", "gateway_upgrade", request.id())
                        .expect("validated upgrade request attribution")
                }),
        )
    } else {
        None
    };
    let audit_error = tokio::time::timeout(
        Duration::from_secs(10),
        audit.record(RetirementRecord {
            request: request.clone(),
            running: running.clone(),
            cleanup_error: cleanup_error.clone(),
            retirement_cause: retirement_cause.clone(),
        }),
    )
    .await
    .unwrap_or_else(|_| Err("retirement audit acknowledgement deadline elapsed".into()))
    .err();
    let retired = cleanup_error.is_none() && audit_error.is_none();
    let refusal = (!retired).then(|| {
        RetirementRefusal::of(
            nothing_left_running && audit_error.is_none(),
            data.missing(),
        )
    });
    RetirementResult {
        request,
        retirement_cause,
        running,
        retired,
        cleanup_error,
        audit_error,
        refusal,
    }
}

/// Restore the original gateway-upgrade fence before serving any product request.
/// Target-generation processes do not inherit the old runtime's retired state.
pub(crate) async fn restore_retirement(
    fence: Option<&RetirementFence>,
    runtime: &RunningRuntime,
    conversations: Option<&ConversationService>,
) -> Result<(), String> {
    if let Some(fence) = fence.filter(|fence| fence.applies_to(runtime)) {
        if let Some(service) = conversations {
            let cause = fence.cause();
            let actor =
                ActionContext::new(cause.principal_id(), cause.surface_id(), cause.request_id())
                    .map_err(|error| error.to_string())?;
            service
                .retire_with_context(actor)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
