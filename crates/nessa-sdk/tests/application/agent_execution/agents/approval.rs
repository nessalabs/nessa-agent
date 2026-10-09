//! A live approval change belongs to the attachment it was verified on.
//! A later attachment opens at the binding's preset, and the mode the agent
//! reports is the mode its admissions record.

use super::MemoryStorage;
use crate::application::agent_execution::support::*;
use std::{sync::atomic::AtomicBool, time::Duration};
use tokio::sync::{
    oneshot::{self, Receiver},
    Notify,
};

/// Records each preset a new context is opened at, and each live change.
struct Backend {
    applied: Mutex<Vec<ApprovalMode>>,
    inner: RecordingSession,
    response: Mutex<Option<Receiver<ProviderOperationResult<()>>>>,
    started: Notify,
    closes: AtomicUsize,
    panic_before_future: AtomicBool,
}
impl ProviderSessionBackend for Backend {
    fn set_approval_mode(&self, mode: ApprovalMode) -> ProviderOperationFuture<'_, ()> {
        assert!(
            !self.panic_before_future.load(Ordering::SeqCst),
            "approval backend panicked constructing its future"
        );
        Box::pin(async move {
            let response = self.response.lock().unwrap().take();
            self.started.notify_one();
            if let Some(response) = response {
                response.await.expect("provider response sender")?;
            }
            self.applied.lock().unwrap().push(mode);
            Ok(())
        })
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
        self.closes.fetch_add(1, Ordering::SeqCst);
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

struct Prepared {
    agent: Agent,
    provider: Arc<Provider>,
    backend: Arc<Backend>,
    audit: Arc<AdmissionAudit>,
    storage: MemoryStorage,
}

async fn prepared() -> Prepared {
    let backend = Arc::new(Backend {
        applied: Mutex::new(Vec::new()),
        response: Mutex::new(None),
        started: Notify::new(),
        closes: AtomicUsize::new(0),
        panic_before_future: AtomicBool::new(false),
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
    let storage = MemoryStorage::default();
    let agent = Agent::prepare(provider.clone(), storage.manager().await, audit.clone())
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
    Prepared {
        agent,
        provider,
        backend,
        audit,
        storage,
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
    let Prepared {
        agent,
        provider,
        backend,
        audit,
        ..
    } = prepared().await;

    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    agent.set_approval_mode(ApprovalMode::Auto).await.unwrap();
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    finish(&agent, "queued-live").await;
    finish_steering(&agent, "steered-live").await;
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));

    agent.close(close_action()).await.unwrap();
    // Detached: the next attachment will open at the binding's preset.
    // Work accepted now records that preset, not the live change, whose
    // provider generation is still the one close left behind.
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    let detached = agent
        .enqueue(request("queued-detached"), close_action())
        .await
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
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), detached.wait())
            .await
            .expect("work admitted while detached runs on the next attachment"),
        Ok(ExecutionOutcome::Completed)
    );
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

/// Close detaches before it takes the scheduler lock, and admission holds that
/// lock across the save. The record is written after the save, so it has to
/// name the preset captured with the permit.
async fn admit_across_close(steer: bool) -> (Result<(), AgentError>, Vec<Option<ApprovalMode>>) {
    let Prepared {
        agent,
        audit,
        storage,
        ..
    } = prepared().await;
    agent.set_approval_mode(ApprovalMode::Auto).await.unwrap();
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    let (saving, release) = storage.pause_invocation_save("across-close");
    let runner = agent.clone();
    let admitted = tokio::spawn(async move {
        if steer {
            runner
                .steer(request("across-close"), close_action())
                .await
                .map(|_| ())
        } else {
            runner
                .enqueue(request("across-close"), close_action())
                .await
                .map(|_| ())
        }
    });
    saving.await.expect("admission reaches its save");
    let closer = agent.clone();
    let closing = tokio::spawn(async move { closer.close(close_action()).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while agent.approval_mode() != Some(ApprovalMode::Ask) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close detaches while the admission save is still paused");
    release.send(()).expect("admission save is still waiting");
    let result = admitted.await.expect("admission task");
    closing.await.expect("close task").expect("close");
    let modes = audit.0.lock().unwrap().clone();
    (result, modes)
}

#[tokio::test]
async fn a_close_during_admission_save_keeps_the_live_mode_on_the_record() {
    let (queued, modes) = admit_across_close(false).await;
    assert_eq!(queued, Ok(()));
    assert_eq!(modes, vec![Some(ApprovalMode::Auto)]);
    // Idle steering reaches the same save, then refuses the queue because close
    // has already detached, and writes no admission record. A record from that
    // path would still have to name the captured preset.
    let (steered, modes) = admit_across_close(true).await;
    assert!(
        matches!(steered, Err(AgentError::Closed)),
        "steering across close was {steered:?}"
    );
    assert!(
        modes.iter().all(|mode| *mode == Some(ApprovalMode::Auto)),
        "steering across close recorded {modes:?}"
    );
}

/// Paused-clock gates establish acceptance before close or caller loss; no sleep
/// creates the interleaving. A held response has independently owned completion.
#[tokio::test(start_paused = true)]
async fn a_pending_approval_change_is_interrupted_by_concurrent_close() {
    let Prepared { agent, backend, .. } = prepared().await;
    let (_release, response) = oneshot::channel();
    *backend.response.lock().unwrap() = Some(response);
    let changing = agent.set_approval_mode(ApprovalMode::Auto);
    tokio::pin!(changing);
    tokio::select! {
        _ = backend.started.notified() => {},
        result = &mut changing => panic!("response must remain pending: {result:?}"),
    }
    tokio::time::timeout(Duration::from_secs(1), agent.close(close_action()))
        .await
        .expect("close interrupts the admitted response wait")
        .unwrap();
    let failure = changing.await.unwrap_err();
    assert_eq!(failure.error(), &AgentError::Closed);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
    assert!(backend.applied.lock().unwrap().is_empty());
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
}

#[tokio::test(start_paused = true)]
async fn a_dropped_approval_caller_still_publishes_its_acknowledged_mode_for_admission() {
    let Prepared {
        agent,
        backend,
        audit,
        ..
    } = prepared().await;
    let (release, response) = oneshot::channel();
    *backend.response.lock().unwrap() = Some(response);
    {
        let changing = agent.set_approval_mode(ApprovalMode::Auto);
        tokio::pin!(changing);
        tokio::select! {
            _ = backend.started.notified() => {},
            result = &mut changing => panic!("response must remain pending: {result:?}"),
        }
        // Drop the actual caller future, not a proxy task waiting on it.
    }
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    release
        .send(Ok(()))
        .expect("owned response survives caller loss");
    finish(&agent, "after-dropped-caller").await;
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(*backend.applied.lock().unwrap(), [ApprovalMode::Auto]);
    assert_eq!(*audit.0.lock().unwrap(), [Some(ApprovalMode::Auto)]);
    agent.set_approval_mode(ApprovalMode::Ask).await.unwrap();
    finish(&agent, "after-repeated-change").await;
    assert_eq!(
        *audit.0.lock().unwrap(),
        [Some(ApprovalMode::Auto), Some(ApprovalMode::Ask)]
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn close_after_approval_caller_loss_retires_a_late_response_without_reviving_the_mode() {
    let Prepared { agent, backend, .. } = prepared().await;
    let (release, response) = oneshot::channel();
    *backend.response.lock().unwrap() = Some(response);
    {
        let changing = agent.set_approval_mode(ApprovalMode::Auto);
        tokio::pin!(changing);
        tokio::select! {
            _ = backend.started.notified() => {},
            result = &mut changing => panic!("response must remain pending: {result:?}"),
        }
    }
    tokio::time::timeout(Duration::from_secs(1), agent.close(close_action()))
        .await
        .expect("close settles after caller loss without the response")
        .unwrap();
    assert!(release.send(Ok(())).is_err(), "stopped response is retired");
    assert_eq!(backend.closes.load(Ordering::SeqCst), 1);
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
    finish(&agent, "after-late-response").await;
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    agent.close(close_action()).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn an_uncertain_approval_response_fences_its_generation_without_publishing_the_mode() {
    let Prepared { agent, backend, .. } = prepared().await;
    let (release, response) = oneshot::channel();
    *backend.response.lock().unwrap() = Some(response);
    let changing = agent.set_approval_mode(ApprovalMode::Auto);
    tokio::pin!(changing);
    tokio::select! {
        _ = backend.started.notified() => {},
        result = &mut changing => panic!("response must remain pending: {result:?}"),
    }
    release
        .send(Err(ProviderOperationFailure::new(
            AgentError::SubmissionUnresolved,
            ProviderSessionState::CleanupRequired,
        )))
        .unwrap();
    let failure = changing.await.unwrap_err();
    assert_eq!(failure.error(), &AgentError::SubmissionUnresolved);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    assert!(backend.applied.lock().unwrap().is_empty());
    assert!(agent
        .invoke(request("uncertain-mode"), close_action())
        .await
        .is_err());
    assert_eq!(backend.inner.prompts.load(Ordering::SeqCst), 0);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn a_backend_panic_before_returning_the_approval_future_fences_its_generation() {
    let Prepared { agent, backend, .. } = prepared().await;
    backend.panic_before_future.store(true, Ordering::SeqCst);
    let failure = agent
        .set_approval_mode(ApprovalMode::Auto)
        .await
        .unwrap_err();
    assert!(matches!(failure.error(), AgentError::Protocol(_)));
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert!(agent
        .invoke(request("after-provider-panic"), close_action())
        .await
        .is_err());
    assert_eq!(backend.inner.prompts.load(Ordering::SeqCst), 0);
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    agent.close(close_action()).await.unwrap();
}
