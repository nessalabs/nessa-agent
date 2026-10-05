//! A live approval change belongs to the attachment it was verified on.
//! A later attachment opens at the binding's preset, and the mode the agent
//! reports is the mode its admissions record.

use super::MemoryStorage;
use crate::application::agent_execution::support::*;

/// Records each preset a new context is opened at, and each live change.
struct Backend {
    applied: Mutex<Vec<ApprovalMode>>,
    inner: RecordingSession,
}
impl ProviderSessionBackend for Backend {
    fn set_approval_mode(&self, mode: ApprovalMode) -> ProviderOperationFuture<'_, ()> {
        self.applied.lock().unwrap().push(mode);
        Box::pin(async { Ok(()) })
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        self.inner.prepare_invocation()
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        self.inner.execute(input)
    }
    fn answer_question(&self, answer: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        self.inner.answer_question(answer)
    }
    fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        self.inner.answer_permission(answer)
    }
    fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        self.inner.cancel_permission(input)
    }
    fn close(&self, origin: SessionCloseRequest) -> CleanupFuture<'_> {
        self.inner.close(origin)
    }
}

struct Events;
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(None) })
    }
}

struct Provider {
    /// The preset every context this binding opens is configured with.
    initial: ApprovalMode,
    opened: Mutex<Vec<ApprovalMode>>,
    backend: Arc<Backend>,
}
impl AgentProvider for Provider {
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.initial)
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("approval-fixture", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        capabilities_ref()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let (_, restore, _) = request.into_parts();
        Box::pin(async move {
            self.opened.lock().unwrap().push(self.initial);
            let id = restore.unwrap_or_else(|| ExecutionSessionId::new("context").unwrap());
            Ok(OpenedProviderSession {
                session: ProviderSession::new(id, self.backend.clone(), capabilities()),
                events: Box::new(Events),
            })
        })
    }
}

/// Keeps the preset each admission record names, in order.
#[derive(Default)]
struct AdmissionAudit(Mutex<Vec<Option<ApprovalMode>>>);
impl ExecutionAudit for AdmissionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if let ExecutionAuditRecord::QueueAdmitted(admitted) = &record {
            self.0.lock().unwrap().push(admitted.approval_mode());
        }
        Box::pin(async { Ok(()) })
    }
}

fn request(id: &str) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(format!("message {id}")).unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 10,
    }
}

async fn finish(agent: &Agent, id: &str) {
    assert_eq!(
        agent
            .enqueue(request(id), close_action())
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
}

async fn finish_steering(agent: &Agent, id: &str) {
    let SteeringDelivery::Queued(receipt) = agent.steer(request(id), close_action()).await.unwrap()
    else {
        panic!("steering while idle is admitted to the queue");
    };
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
}

#[tokio::test]
async fn a_live_approval_change_is_what_its_attachment_admits_and_a_new_attachment_admits_the_binding_mode(
) {
    let backend = Arc::new(Backend {
        applied: Mutex::new(Vec::new()),
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
    });
    let provider = Arc::new(Provider {
        initial: ApprovalMode::Ask,
        opened: Mutex::new(Vec::new()),
        backend: backend.clone(),
    });
    let audit = Arc::new(AdmissionAudit::default());
    let manager = MemoryStorage::default().manager().await;
    let agent = Agent::prepare(provider.clone(), manager, audit.clone())
        .await
        .map_err(|error| error.cause().clone())
        .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();

    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    agent.set_approval_mode(ApprovalMode::Auto).await.unwrap();
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    finish(&agent, "queued-live").await;
    finish_steering(&agent, "steered-live").await;
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));

    agent.close(close_action()).await.unwrap();
    // Detached: the next attachment will open at the binding's preset.
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();

    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    finish(&agent, "queued-next").await;
    finish_steering(&agent, "steered-next").await;
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    assert_eq!(
        *audit.0.lock().unwrap(),
        [
            Some(ApprovalMode::Auto),
            Some(ApprovalMode::Auto),
            Some(ApprovalMode::Ask),
            Some(ApprovalMode::Ask),
        ]
    );
    // The new context was opened at the binding's preset, not the live change.
    assert_eq!(
        *provider.opened.lock().unwrap(),
        [ApprovalMode::Ask, ApprovalMode::Ask]
    );
    assert_eq!(*backend.applied.lock().unwrap(), [ApprovalMode::Auto]);
    agent.close(close_action()).await.unwrap();
}
