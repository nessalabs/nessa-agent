//! Public same-provider control preserves an active execution and exact saved context.
use nessa_sdk::application::agent_execution::providers::NativeExecutionHandoff;
mod admission;
mod hooks;
mod steering;
mod supervision;
use nessa_sdk::application::agent_execution::traces::LocalTraceStore;
use nessa_sdk::application::dto::ImageInputLimitsDto;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use nessa_sdk::domain::model_metadata::value_objects::EffortLevel;

use super::MemoryStorage;
use crate::application::agent_execution::support::*;
use nessa_sdk::application::agent_execution::providers::{
    HarnessHost, HarnessHostIdentity, HarnessLaunch, HarnessProcess,
};
use std::{sync::atomic::AtomicBool, time::Duration};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::timeout;

struct Backend {
    inner: RecordingSession,
    image_input: bool,
    hold: Mutex<Option<oneshot::Receiver<()>>>,
    started: Notify,
    preparation_gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    closes: AtomicUsize,
    fail_model: Mutex<Option<AgentError>>,
    execution_failure: Mutex<Option<ProviderOperationFailure>>,
    fail_approval: Mutex<Option<AgentError>>,
    replayed_failure: Mutex<Option<ProviderOperationFailure>>,
    model_mode: Mutex<ApprovalMode>,
    model_timing: Mutex<ModelChangeTiming>,
    steering: Mutex<Option<SteeringOutcome>>,
    steered: Mutex<Vec<(ExecutionId, ExecutionRequest)>>,
    model_effort: Mutex<Option<EffortLevel>>,
    model_calls: AtomicUsize,
    model_gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    approval_calls: AtomicUsize,
    effort_calls: AtomicUsize,
    events: Mutex<Option<mpsc::Sender<ExecutionEvent>>>,
    audit: Arc<Mutex<Vec<ExecutionAuditRecord>>>,
    fail_terminal_audit: Arc<AtomicBool>,
}
impl ProviderSessionBackend for Backend {
    fn validate_input(
        &self,
        input: &ExecutionRequest,
        capabilities: &EffectiveCapabilities,
    ) -> Result<(), AgentError> {
        if input.estimated_input_tokens + u64::from(input.reserved_output_tokens)
            > u64::from(capabilities.limits().max_context_window())
            || input.reserved_output_tokens > capabilities.limits().max_output()
        {
            return Err(AgentError::InvalidInput(
                "model-aware backend input limits".into(),
            ));
        }
        Ok(())
    }

    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        ProviderOperationCapabilities {
            negotiated: true,
            image_input: self.image_input,
            native_steering: self.steering.lock().unwrap().is_some(),
            effort_levels: capabilities_for("small", 200)
                .effort_levels()
                .unwrap()
                .offered_by(["low", "high"]),
            ..ProviderOperationCapabilities::default()
        }
    }
    fn set_effort_level(&self, _level: EffortLevel) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            self.effort_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn set_model(
        &self,
        change: ModelChange,
        capabilities: EffectiveCapabilities,
    ) -> ProviderOperationFuture<'_, ModelChangeEffect> {
        Box::pin(async move {
            self.model_calls.fetch_add(1, Ordering::SeqCst);
            let gate = self.model_gate.lock().unwrap().take();
            if let Some((started, release)) = gate {
                started.send(()).unwrap();
                release.await.unwrap();
            }
            if let Some(failure) = self.replayed_failure.lock().unwrap().take() {
                return Err(failure);
            }
            if let Some(error) = self.fail_model.lock().unwrap().take() {
                return Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::CleanupRequired,
                ));
            }
            Ok(ModelChangeEffect::new(
                change.after().clone(),
                capabilities,
                Some(*self.model_mode.lock().unwrap()),
                self.model_effort.lock().unwrap().clone(),
                *self.model_timing.lock().unwrap(),
            ))
        })
    }
    fn set_approval_mode(&self, _mode: ApprovalMode) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async {
            self.approval_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(failure) = self.replayed_failure.lock().unwrap().take() {
                return Err(failure);
            }
            if let Some(error) = self.fail_approval.lock().unwrap().take() {
                return Err(ProviderOperationFailure::new(
                    error,
                    ProviderSessionState::Usable,
                ));
            }
            Ok(())
        })
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move {
            let gate = self.preparation_gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            self.inner.prepare_invocation().await
        })
    }
    fn execute(
        &self,
        input: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            // This fixture owns its native turn before announcing started;
            // the inner backend below supplies its terminal result.
            if let Err(error) = handoff() {
                return ProviderExecutionReply::Rejected(error);
            }
            if let Some(failure) = self.execution_failure.lock().unwrap().take() {
                self.events.lock().unwrap().take();
                let (error, state) = failure.into_parts();
                return ProviderExecutionReply::Finished(ExecutionReport::new(
                    Some(Err(error)),
                    None,
                    state,
                ));
            }
            let hold = self.hold.lock().unwrap().take();
            self.started.notify_one();
            if let Some(hold) = hold {
                let _ = hold.await;
            }
            let sender = self.events.lock().unwrap().clone();
            if let Some(sender) = sender {
                sender
                    .send(ExecutionEvent::new(
                        input.execution_id.clone(),
                        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                    ))
                    .await
                    .unwrap();
            }
            self.inner.execute(input, handoff).await
        })
    }
    fn steer<'a>(
        &'a self,
        target: ExecutionId,
        input: ExecutionRequest,
        _capabilities: &'a EffectiveCapabilities,
        started: &'a (dyn Fn() + Send + Sync),
    ) -> ProviderOperationFuture<'a, SteeringOutcome> {
        Box::pin(async move {
            started();
            let outcome = *self.steering.lock().unwrap();
            match outcome {
                Some(outcome) => {
                    self.steered.lock().unwrap().push((target, input));
                    Ok(outcome)
                }
                None => Err(ProviderOperationFailure::new(
                    AgentError::Unsupported("fixture native steering".into()),
                    ProviderSessionState::Usable,
                )),
            }
        })
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
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.inner.close(request)
    }
}
struct Events(mpsc::Receiver<ExecutionEvent>);
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async { Ok(self.0.recv().await) })
    }
}
#[derive(Clone, Copy)]
enum Normalization {
    Exact,
    PanicAdjusted,
    PanicPreflight,
    PanicRetained,
    PanicHostileAdjusted,
    IgnoreMode,
    AskOnly,
    IgnoreEffort,
    ChangeHost,
    ChangeHostAdjusted,
    ChangeHooks,
    ChangeHooksAdjusted,
}
#[derive(Clone)]
struct Provider {
    model: String,
    mode: ApprovalMode,
    normalization_failure: bool,
    native_hooks: bool,
    opened_hooks: Arc<Mutex<Vec<bool>>>,
    normalization: Normalization,
    normalization_calls: Arc<AtomicUsize>,
    payload_drops: Arc<AtomicUsize>,
    host: Option<Arc<dyn HarnessHost>>,
    normalization_host: Arc<dyn HarnessHost>,
    effort: Option<EffortLevel>,
    context: String,
    capabilities: EffectiveCapabilities,
    backend: Arc<Backend>,
    restores: Arc<Mutex<Vec<Option<ExecutionSessionId>>>>,
    /// Set for a provider that starts where no saved context is: it
    /// restores none, and the context it opens is named this.
    elsewhere: Option<&'static str>,
}
impl AgentProvider for Provider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new(
            "model-fixture",
            &self.model,
            format!("{}:{}", self.context, self.model),
        )
        .unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.mode)
    }
    fn native_hooks_allowed(&self) -> bool {
        self.native_hooks
    }
    fn restores_saved_context(&self, _saved: &ExecutionSessionId) -> bool {
        self.elsewhere.is_none()
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.effort.clone()
    }
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.host
            .as_ref()
            .map_or_else(HarnessHostIdentity::local, HarnessHostIdentity::hosted)
    }
    fn model_change_context(&self) -> Option<String> {
        Some(self.context.clone())
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        _traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        if matches!(self.normalization, Normalization::PanicRetained)
            && self.normalization_calls.fetch_add(1, Ordering::SeqCst) == 1
        {
            panic!("retention factory panic");
        }
        if matches!(self.normalization, Normalization::PanicHostileAdjusted)
            && mode != self.approval_mode()
        {
            supervision::hostile_payload(self.payload_drops.clone());
        }
        if matches!(self.normalization, Normalization::PanicPreflight) {
            panic!("preflight factory panic");
        }
        if matches!(self.normalization, Normalization::PanicAdjusted)
            && mode != self.approval_mode()
        {
            panic!("fixture panics only when normalizing adjusted ACK");
        }
        if matches!(self.normalization, Normalization::AskOnly) && mode != Some(ApprovalMode::Ask) {
            return Err(AgentError::Unsupported("target accepts Ask only".into()));
        }
        if self.normalization_failure {
            return Err(AgentError::Unsupported(
                "fixture cannot retain these settings".into(),
            ));
        }
        let change_host = matches!(self.normalization, Normalization::ChangeHost)
            || (matches!(self.normalization, Normalization::ChangeHostAdjusted)
                && (mode != self.approval_mode() || effort != self.effort_level()));
        let change_hooks = matches!(self.normalization, Normalization::ChangeHooks)
            || (matches!(self.normalization, Normalization::ChangeHooksAdjusted)
                && (mode != self.approval_mode() || effort != self.effort_level()));
        Ok(Arc::new(Self {
            native_hooks: if change_hooks {
                !self.native_hooks
            } else {
                self.native_hooks
            },
            host: if change_host {
                Some(self.normalization_host.clone())
            } else {
                self.host.clone()
            },
            mode: if matches!(self.normalization, Normalization::IgnoreMode) {
                self.mode
            } else {
                mode.unwrap()
            },
            effort: if matches!(self.normalization, Normalization::IgnoreEffort) {
                self.effort.clone()
            } else {
                effort
            },
            ..self.clone()
        }))
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let (_, restore, _) = request.into_parts();
        Box::pin(async move {
            self.restores.lock().unwrap().push(restore.clone());
            self.opened_hooks.lock().unwrap().push(self.native_hooks);
            let (sender, receiver) = mpsc::channel(4);
            *self.backend.events.lock().unwrap() = Some(sender);
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    restore.unwrap_or_else(|| {
                        ExecutionSessionId::new(self.elsewhere.unwrap_or("same-native-context"))
                            .unwrap()
                    }),
                    self.backend.clone(),
                    self.capabilities.clone(),
                ),
                events: Box::new(Events(receiver)),
            })
        })
    }
}
fn capabilities_for(id: &str, maximum: u32) -> EffectiveCapabilities {
    capabilities_with_images(id, maximum, None)
}
fn capabilities_with_images(
    id: &str,
    maximum: u32,
    image_input: Option<ImageInputLimitsDto>,
) -> EffectiveCapabilities {
    capabilities_with_limits(id, maximum, 100, image_input)
}
fn capabilities_with_limits(
    id: &str,
    maximum: u32,
    maximum_output: u32,
    image_input: Option<ImageInputLimitsDto>,
) -> EffectiveCapabilities {
    let images = image_input.is_some();
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: id.into(),
        display_name: id.into(),
        input: ModalitiesDto {
            image: images,
            ..text
        },
        image_input,
        output: text,
        tool_use: true,
        reasoning: Some(nessa_sdk::application::dto::ReasoningDto {
            effort_levels: vec!["low".into(), "high".into()],
        }),
        fast_mode: false,
        max_context_window_tokens: maximum,
        max_output_tokens: maximum_output,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(
            ModelFeatures::new(
                Modalities::new(true, images, false).unwrap(),
                text,
                true,
                true,
                false,
            ),
            model.limits(),
        ),
        model.limits(),
    )
    .unwrap()
}
fn target(prior: &Provider) -> Arc<Provider> {
    Arc::new(Provider {
        model: "small".into(),
        capabilities: capabilities_for("small", 200),
        ..prior.clone()
    })
}
fn input(id: &str, tokens: u64) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(id.to_owned()).unwrap()),
        estimated_input_tokens: tokens,
        reserved_output_tokens: 10,
    }
}
struct UnstartedHost;
impl HarnessHost for UnstartedHost {
    fn workspace(&self) -> &std::path::Path {
        std::path::Path::new(if cfg!(windows) {
            r"C:\same-workspace"
        } else {
            "/same-workspace"
        })
    }
    fn start(&self, _launch: HarnessLaunch) -> Result<HarnessProcess, AgentError> {
        Err(AgentError::Unsupported("identity-only fixture host".into()))
    }
}
async fn prepared(hold: Option<oneshot::Receiver<()>>) -> (Agent, Arc<Provider>, MemoryStorage) {
    prepared_with_normalization(hold, Normalization::Exact, None).await
}
async fn prepared_with_normalization(
    hold: Option<oneshot::Receiver<()>>,
    normalization: Normalization,
    host: Option<Arc<dyn HarnessHost>>,
) -> (Agent, Arc<Provider>, MemoryStorage) {
    prepared_with_capabilities(hold, normalization, host, capabilities_for("large", 1000)).await
}
async fn prepared_with_capabilities(
    hold: Option<oneshot::Receiver<()>>,
    normalization: Normalization,
    host: Option<Arc<dyn HarnessHost>>,
    capabilities: EffectiveCapabilities,
) -> (Agent, Arc<Provider>, MemoryStorage) {
    let backend = Arc::new(Backend {
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
        image_input: capabilities.features().input().image(),
        hold: Mutex::new(hold),
        started: Notify::new(),
        preparation_gate: Mutex::new(None),
        closes: AtomicUsize::new(0),
        fail_model: Mutex::new(None),
        execution_failure: Mutex::new(None),
        fail_approval: Mutex::new(None),
        replayed_failure: Mutex::new(None),
        model_mode: Mutex::new(ApprovalMode::Ask),
        model_timing: Mutex::new(ModelChangeTiming::NextTurn),
        steering: Mutex::new(None),
        steered: Mutex::new(Vec::new()),
        model_effort: Mutex::new(None),
        model_calls: AtomicUsize::new(0),
        model_gate: Mutex::new(None),
        approval_calls: AtomicUsize::new(0),
        effort_calls: AtomicUsize::new(0),
        events: Mutex::new(None),
        audit: Arc::new(Mutex::new(Vec::new())),
        fail_terminal_audit: Arc::new(AtomicBool::new(false)),
    });
    let provider = Arc::new(Provider {
        model: "large".into(),
        mode: ApprovalMode::Ask,
        normalization_failure: false,
        native_hooks: false,
        opened_hooks: Arc::new(Mutex::new(Vec::new())),
        normalization,
        normalization_calls: Arc::new(AtomicUsize::new(0)),
        payload_drops: Arc::new(AtomicUsize::new(0)),
        host,
        normalization_host: Arc::new(UnstartedHost),
        effort: None,
        context: "workspace".into(),
        capabilities,
        backend,
        restores: Arc::new(Mutex::new(Vec::new())),
        elsewhere: None,
    });
    let storage = MemoryStorage::default();
    struct Audit(Arc<Mutex<Vec<ExecutionAuditRecord>>>, Arc<AtomicBool>);
    impl ExecutionAudit for Audit {
        fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
            let fail = matches!(&record, ExecutionAuditRecord::ConfigurationControlled(record)
                if record.stage() == ConfigurationControlStage::Failed)
                && self.1.load(Ordering::SeqCst);
            self.0.lock().unwrap().push(record);
            Box::pin(async move {
                if fail {
                    Err(AgentError::AuditFailure)
                } else {
                    Ok(())
                }
            })
        }
    }
    let agent = Agent::prepare(
        provider.clone(),
        storage.manager().await,
        Arc::new(Audit(
            provider.backend.audit.clone(),
            provider.backend.fail_terminal_audit.clone(),
        )),
    )
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
    (agent, provider, storage)
}
/// A provider that starts where the saved context is not (another worker
/// than the one that opened it) is asked to restore nothing: attaching opens
/// new context and records it in place of the saved one, which the next
/// provider that does hold it restores.
#[tokio::test]
async fn a_provider_that_does_not_hold_the_saved_context_opens_new_context_in_its_place() {
    let (agent, prior, storage) = prepared(None).await;
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(agent);
    let elsewhere = Arc::new(Provider {
        elsewhere: Some("context-on-another-worker"),
        ..(*prior).clone()
    });
    let moved = attached_agent(elsewhere.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(elsewhere.restores.lock().unwrap().last(), Some(&None));
    assert_eq!(
        moved
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_context
            .recorded()
            .unwrap()
            .as_str(),
        "context-on-another-worker"
    );
    timeout(Duration::from_secs(3), moved.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(moved);
    let holding = Arc::new(Provider {
        elsewhere: None,
        ..(*prior).clone()
    });
    let reopened = attached_agent(holding.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(
        holding
            .restores
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_str(),
        "context-on-another-worker"
    );
    timeout(Duration::from_secs(3), reopened.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
}
/// A provider that does not restore the saved context goes on under its own
/// context fingerprint, which the history records in place of the saved one;
/// a later provider with that fingerprint that does restore the context
/// reopens it, and one still carrying the replaced fingerprint is refused.
#[tokio::test]
async fn a_provider_that_does_not_hold_the_saved_context_records_its_own_fingerprint() {
    let (agent, prior, storage) = prepared(None).await;
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(agent);
    let elsewhere = Arc::new(Provider {
        context: "another-workspace".into(),
        elsewhere: Some("context-on-another-worker"),
        ..(*prior).clone()
    });
    assert_ne!(elsewhere.identity(), prior.identity());
    assert_eq!(elsewhere.identity().name(), prior.identity().name());
    assert_eq!(elsewhere.identity().model_id(), prior.identity().model_id());
    let moved = attached_agent(elsewhere.clone(), storage.manager().await)
        .await
        .expect("a provider that restores nothing goes on under its own fingerprint");
    assert_eq!(elsewhere.restores.lock().unwrap().last(), Some(&None));
    let snapshot = moved.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.provider, elsewhere.identity());
    assert_eq!(
        snapshot.provider_context.recorded().unwrap().as_str(),
        "context-on-another-worker"
    );
    timeout(Duration::from_secs(3), moved.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(moved);
    assert_eq!(
        storage
            .0
            .lock()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .provider,
        elsewhere.identity()
    );

    let stale = Arc::new(Provider {
        elsewhere: None,
        ..(*prior).clone()
    });
    let writes = storage.writes();
    match attached_agent(stale, storage.manager().await).await {
        Ok(_) => panic!("a restore under the replaced fingerprint is refused"),
        Err(error) => assert_eq!(error, AgentError::Storage(StorageError::IdentityMismatch)),
    }
    assert_eq!(storage.writes(), writes);

    let holding = Arc::new(Provider {
        elsewhere: None,
        ..(*elsewhere).clone()
    });
    let reopened = attached_agent(holding.clone(), storage.manager().await)
        .await
        .expect("the recorded fingerprint reopens");
    assert_eq!(
        holding
            .restores
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_str(),
        "context-on-another-worker"
    );
    assert_eq!(
        reopened
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider,
        elsewhere.identity()
    );
    timeout(Duration::from_secs(3), reopened.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
}
/// Only the context fingerprint may change under a provider that does not
/// restore the saved context: another provider name or model is refused
/// before anything is saved, and the history keeps its identity.
#[tokio::test]
async fn a_provider_that_does_not_hold_the_saved_context_cannot_change_name_or_model() {
    let (agent, prior, storage) = prepared(None).await;
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(agent);
    let saved_identity = prior.identity();
    let renamed = ProviderIdentity::new(
        "another-fixture",
        saved_identity.model_id(),
        format!("another-workspace:{}", saved_identity.model_id()),
    )
    .unwrap();
    storage
        .0
        .lock()
        .unwrap()
        .snapshot
        .as_mut()
        .unwrap()
        .provider = renamed.clone();
    let elsewhere = Arc::new(Provider {
        context: "another-workspace".into(),
        elsewhere: Some("context-on-another-worker"),
        ..(*prior).clone()
    });
    assert_eq!(elsewhere.identity().model_id(), renamed.model_id());
    assert_ne!(elsewhere.identity().name(), renamed.name());
    let saved = storage.0.lock().unwrap().snapshot.clone();
    let writes = storage.writes();
    match attached_agent(elsewhere.clone(), storage.manager().await).await {
        Ok(_) => panic!("another provider name is refused"),
        Err(error) => assert_eq!(error, AgentError::Storage(StorageError::IdentityMismatch)),
    }
    assert_eq!(storage.writes(), writes);
    assert_eq!(storage.0.lock().unwrap().snapshot, saved);

    storage
        .0
        .lock()
        .unwrap()
        .snapshot
        .as_mut()
        .unwrap()
        .provider = saved_identity.clone();
    let remodelled = Arc::new(Provider {
        elsewhere: Some("context-on-another-worker"),
        ..(*target(&prior)).clone()
    });
    assert_eq!(remodelled.identity().name(), saved_identity.name());
    assert_ne!(remodelled.identity().model_id(), saved_identity.model_id());
    let saved = storage.0.lock().unwrap().snapshot.clone();
    match attached_agent(remodelled, storage.manager().await).await {
        Ok(_) => panic!("another model is refused"),
        Err(error) => assert_eq!(error, AgentError::Storage(StorageError::IdentityMismatch)),
    }
    assert_eq!(storage.writes(), writes);
    assert_eq!(storage.0.lock().unwrap().snapshot, saved);
    assert_eq!(
        storage
            .0
            .lock()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .provider,
        saved_identity
    );
}
/// A cancellation names the provider context in force when it was recorded,
/// so a review cancelled under the first context does not stop the
/// conversation from moving: the new context is recorded in its place, the
/// cancellation keeps naming the first, and the conversation reopens.
#[tokio::test]
async fn a_review_cancelled_before_the_move_does_not_stop_the_conversation_moving() {
    let (agent, prior, storage) = prepared(None).await;
    agent
        .invoke(input("reviewed", 400), Submitter::owner(close_action()))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(agent);
    let first = ExecutionSessionId::new("same-native-context").unwrap();
    {
        let mut state = storage.0.lock().unwrap();
        let saved = state.snapshot.as_mut().unwrap();
        assert_eq!(saved.provider_context.recorded(), Some(&first));
        let execution = saved.invocations[0].request.execution_id.clone();
        let id = PermissionId::new("review").unwrap();
        let tool = ToolCallId::new("tool").unwrap();
        let input = ToolReviewInput {
            name: "Read".into(),
            arguments_json: "{}".into(),
        };
        let options = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap();
        let mut aggregate = ExecutionSession::new(first.clone());
        aggregate.begin_execution(execution.clone()).unwrap();
        aggregate
            .observe_tool(
                &execution,
                ToolCallUpdate::new(tool.clone(), None, None, None, None, None),
            )
            .unwrap();
        aggregate
            .request_permission(PermissionRequest::new(
                id.clone(),
                execution.clone(),
                tool.clone(),
                options.clone(),
            ))
            .unwrap();
        let record = aggregate
            .cancel_permission(
                &execution,
                &id,
                PermissionCancellationReason::provider_withdrawal(),
            )
            .unwrap()
            .unwrap();
        let cancellation = PermissionCancellation::from_record(
            first.clone(),
            record,
            input.clone(),
            CancellationOrigin::Provider,
        )
        .unwrap();
        for (at, update) in [
            ExecutionUpdate::PermissionRequested {
                id,
                tool_id: tool,
                observation: ToolObservation::default(),
                input,
                options,
                deadline: None,
            },
            ExecutionUpdate::PermissionCancelled(cancellation),
        ]
        .into_iter()
        .enumerate()
        {
            saved.invocations[0]
                .events
                .insert(at, ExecutionEvent::new(execution.clone(), update));
        }
    }
    let cancelled = |snapshot: &SessionSnapshot| {
        snapshot.invocations[0].events.iter().any(|event| {
            matches!(
                event.update(),
                ExecutionUpdate::PermissionCancelled(record) if record.session_id() == &first
            )
        })
    };
    let elsewhere = Arc::new(Provider {
        elsewhere: Some("context-on-another-worker"),
        ..(*prior).clone()
    });
    let moved = attached_agent(elsewhere, storage.manager().await)
        .await
        .expect("a cancelled review does not stop the move");
    let snapshot = moved.session_manager().snapshot().await.unwrap();
    assert_eq!(
        snapshot.provider_context.recorded().unwrap().as_str(),
        "context-on-another-worker"
    );
    assert!(cancelled(&snapshot));
    timeout(Duration::from_secs(3), moved.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(moved);
    let holding = Arc::new(Provider {
        elsewhere: None,
        ..(*prior).clone()
    });
    let reopened = attached_agent(holding.clone(), storage.manager().await)
        .await
        .expect("the moved conversation reopens");
    assert_eq!(
        holding
            .restores
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_str(),
        "context-on-another-worker"
    );
    assert!(cancelled(
        &reopened.session_manager().snapshot().await.unwrap()
    ));
    timeout(Duration::from_secs(3), reopened.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
}

#[tokio::test]
async fn a_live_model_change_keeps_the_active_turn_and_context_and_changes_later_admission() {
    let (release, hold) = oneshot::channel();
    let (agent, prior, storage) = prepared(Some(hold)).await;
    let active = agent
        .enqueue(input("original", 400), Submitter::owner(close_action()))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .expect("original native turn starts");
    let active_id = agent.active_execution_id();
    let chosen = target(&prior);
    let effect = timeout(
        Duration::from_secs(3),
        agent.set_model(chosen.clone(), close_action()),
    )
    .await
    .expect("model control settles while turn active")
    .unwrap();
    assert_eq!(effect.identity(), &chosen.identity());
    assert_eq!(effect.timing(), ModelChangeTiming::NextTurn);
    assert_eq!(agent.active_execution_id(), active_id);
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 0);
    assert_eq!(agent.provider_identity(), chosen.identity());
    assert!(agent
        .enqueue(input("too-big", 400), Submitter::owner(close_action()))
        .await
        .is_err());
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), active.wait())
            .await
            .expect("original turn settles after release"),
        Ok(ExecutionOutcome::Completed)
    );
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.provider, chosen.identity());
    assert_eq!(
        snapshot.provider_context.recorded().unwrap().as_str(),
        "same-native-context"
    );
    assert_eq!(snapshot.invocations.len(), 1);
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(agent);
    let reopened = attached_agent(chosen, storage.manager().await)
        .await
        .unwrap();
    assert_eq!(
        prior
            .restores
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .as_str(),
        "same-native-context"
    );
    assert_eq!(
        reopened
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations
            .len(),
        1
    );
    timeout(Duration::from_secs(3), reopened.close(close_action()))
        .await
        .expect("reopened close settles")
        .unwrap();
}
#[tokio::test]
async fn another_execution_context_is_refused_without_mutation() {
    let (agent, prior, _) = prepared(None).await;
    let wrong = Arc::new(Provider {
        context: "another-workspace".into(),
        ..(*target(&prior)).clone()
    });
    assert!(agent.set_model(wrong, close_action()).await.is_err());
    assert_eq!(agent.provider_identity(), prior.identity());
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 0);
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
}
#[tokio::test]
async fn a_failed_model_control_never_publishes_the_requested_model() {
    let (agent, prior, _) = prepared(None).await;
    *prior.backend.fail_model.lock().unwrap() = Some(AgentError::Protocol("lost response".into()));
    let failure = agent
        .set_model(target(&prior), close_action())
        .await
        .unwrap_err();
    assert_ne!(failure.session_state(), &ProviderSessionState::Usable);
    assert_eq!(agent.provider_identity(), prior.identity());
    assert_eq!(
        agent.session_manager().snapshot().await.unwrap().provider,
        prior.identity()
    );
    let _ = agent.close(close_action()).await;
}

#[tokio::test]
async fn a_later_approval_choice_restores_current_settings_without_rewriting_model_history() {
    let (agent, prior, storage) = prepared(None).await;
    let chosen = target(&prior);
    agent
        .set_model(chosen.clone(), close_action())
        .await
        .unwrap();
    let actor = ActionContext::new("person", "window", "later-approval").unwrap();
    agent
        .set_approval_mode(ApprovalMode::Auto, actor.clone())
        .await
        .unwrap();
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .model_change
            .as_ref()
            .unwrap()
            .configuration()
            .approval_mode(),
        Some(ApprovalMode::Ask)
    );
    let current = snapshot.current_configuration.as_ref().unwrap();
    assert_eq!(current.kind(), SessionConfigurationKind::ApprovalMode);
    assert_eq!(current.actor(), &actor);
    assert_eq!(current.after().approval_mode(), Some(ApprovalMode::Auto));
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let reopened = attached_agent(chosen, storage.manager().await)
        .await
        .unwrap();
    assert_eq!(reopened.approval_mode(), Some(ApprovalMode::Auto));
    reopened.close(close_action()).await.unwrap();
}
#[tokio::test]
async fn model_storage_failure_fences_the_effect_without_publishing_a_committed_choice() {
    let (agent, prior, storage) = prepared(None).await;
    {
        let mut saved = storage.0.lock().unwrap();
        saved.fail_write = Some(saved.writes + 1);
    }
    let failed = agent
        .set_model(target(&prior), close_action())
        .await
        .unwrap_err();
    assert_ne!(failed.session_state(), &ProviderSessionState::Usable);
    assert_eq!(failed.stage(), ProviderOperationStage::EffectUncertain);
    assert_eq!(
        agent.session_manager().snapshot().await.unwrap().provider,
        prior.identity()
    );
    let _ = agent.close(close_action()).await;
}
#[tokio::test]
async fn local_context_refusal_has_owner_issued_before_effect_evidence() {
    let (agent, prior, _) = prepared(None).await;
    let wrong = Arc::new(Provider {
        context: "different".into(),
        ..(*target(&prior)).clone()
    });
    let refused = agent.set_model(wrong, close_action()).await.unwrap_err();
    assert_eq!(refused.stage(), ProviderOperationStage::BeforeEffect);
    assert_eq!(refused.session_state(), &ProviderSessionState::Usable);
    assert_eq!(
        ProviderOperationFailure::new(
            AgentError::Unsupported("backend diagnostic".into()),
            ProviderSessionState::Usable
        )
        .stage(),
        ProviderOperationStage::EffectUncertain
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn active_checked_turn_refuses_relaxed_controls_before_dispatch() {
    let (release, hold) = oneshot::channel();
    let (agent, prior, _) = prepared(Some(hold)).await;
    let submitter = Submitter::new(
        close_action(),
        Initiation::checked_grant(GrantInitiator::new("driver", 7).unwrap()),
    );
    let receipt = agent.enqueue(input("checked", 1), submitter).await.unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .unwrap();
    let id = agent.active_execution_id();
    let refusal = agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap_err();
    assert_eq!(refusal.stage(), ProviderOperationStage::BeforeEffect);
    assert_eq!(prior.backend.approval_calls.load(Ordering::SeqCst), 0);
    let mut relaxed = (*target(&prior)).clone();
    relaxed.mode = ApprovalMode::Auto;
    let refusal = agent
        .set_model(Arc::new(relaxed), close_action())
        .await
        .unwrap_err();
    assert_eq!(refusal.stage(), ProviderOperationStage::BeforeEffect);
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(agent.active_execution_id(), id);
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    release.send(()).unwrap();
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn native_model_adjustment_cannot_relax_a_running_checked_turn() {
    let (release, hold) = oneshot::channel();
    let (agent, prior, _) = prepared(Some(hold)).await;
    let submitter = Submitter::new(
        close_action(),
        Initiation::checked_grant(GrantInitiator::new("driver", 7).unwrap()),
    );
    let receipt = agent
        .enqueue(input("checked-ack", 1), submitter)
        .await
        .unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .unwrap();
    *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Full;
    let failure = agent
        .set_model(target(&prior), close_action())
        .await
        .unwrap_err();
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(agent.provider_identity(), prior.identity());
    assert!(agent
        .enqueue(input("unsafe-next", 1), Submitter::owner(close_action()))
        .await
        .is_err());
    let _ = release.send(());
    drop(receipt);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn owner_can_change_permission_mode_during_a_held_turn_without_retiring_it() {
    let (release, hold) = oneshot::channel();
    let (agent, prior, _) = prepared(Some(hold)).await;
    let receipt = agent
        .enqueue(input("owner-active", 1), Submitter::owner(close_action()))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .unwrap();
    let id = agent.active_execution_id();
    agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap();
    assert_eq!(agent.active_execution_id(), id);
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn contradictory_target_model_capabilities_are_refused_before_native_dispatch() {
    let (agent, prior, _) = prepared(None).await;
    let mut contradictory = (*target(&prior)).clone();
    contradictory.capabilities = capabilities_for("someone-else", 200);
    let failure = agent
        .set_model(Arc::new(contradictory), close_action())
        .await
        .unwrap_err();
    assert_eq!(failure.stage(), ProviderOperationStage::BeforeEffect);
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(agent.provider_identity(), prior.identity());
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn an_unchanged_model_is_refused_without_claiming_a_native_change() {
    let (agent, prior, _) = prepared(None).await;
    let failure = agent
        .set_model(prior.clone(), close_action())
        .await
        .unwrap_err();
    assert_eq!(failure.stage(), ProviderOperationStage::BeforeEffect);
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .model_change,
        None
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn an_explicit_cold_model_change_retains_context_history_and_exact_attribution() {
    let (agent, prior, storage) = prepared(None).await;
    agent
        .enqueue(
            input("saved-before-cold-choice", 1),
            Submitter::owner(close_action()),
        )
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let original = agent.session_manager().snapshot().await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let target = target(&prior);
    let change = ModelChange::between(prior.as_ref(), target.as_ref()).unwrap();
    let actor = close_action();
    let mut manager = storage.manager().await;
    manager
        .change_model(change.clone(), actor.clone())
        .await
        .unwrap();
    let saved = manager.saved_model_change().await.unwrap().unwrap();
    assert_eq!(saved.change(), &change);
    assert_eq!(saved.actor(), &actor);
    assert_eq!(saved.timing(), None);
    let reopened = attached_agent(target, manager).await.unwrap();
    let selected = reopened.session_manager().snapshot().await.unwrap();
    assert_eq!(selected.provider_context, original.provider_context);
    assert_eq!(selected.invocations, original.invocations);
    reopened.close(close_action()).await.unwrap();
}

/// A conversation whose model was changed can still move: the last model
/// change chose the name and model, and the move keeps both while it
/// replaces the context fingerprint, so the moved history reopens there.
#[tokio::test]
async fn a_conversation_whose_model_changed_still_moves_and_reopens() {
    let (agent, prior, storage) = prepared(None).await;
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let target = target(&prior);
    let change = ModelChange::between(prior.as_ref(), target.as_ref()).unwrap();
    let mut manager = storage.manager().await;
    manager.change_model(change, close_action()).await.unwrap();
    attached_agent(target.clone(), manager)
        .await
        .unwrap()
        .close(close_action())
        .await
        .unwrap();

    let elsewhere = Arc::new(Provider {
        context: "another-workspace".into(),
        elsewhere: Some("context-on-another-worker"),
        ..(*target).clone()
    });
    let moved = attached_agent(elsewhere.clone(), storage.manager().await)
        .await
        .expect("a model-changed conversation moves");
    timeout(Duration::from_secs(3), moved.close(close_action()))
        .await
        .expect("agent close settles")
        .unwrap();
    drop(moved);

    let holding = Arc::new(Provider {
        elsewhere: None,
        ..(*elsewhere).clone()
    });
    let reopened = attached_agent(holding, storage.manager().await)
        .await
        .expect("the moved history reopens");
    let snapshot = reopened.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.provider, elsewhere.identity());
    assert_eq!(
        snapshot.model_change.unwrap().change().after(),
        &target.identity()
    );
    reopened.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn cold_rebinding_refuses_an_inexact_prior_identity() {
    let (agent, prior, storage) = prepared(None).await;
    let original = agent.session_manager().snapshot().await.unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let target = target(&prior);
    let reversed = ModelChange::between(target.as_ref(), prior.as_ref()).unwrap();
    let failure = storage
        .manager()
        .await
        .change_model(reversed, close_action())
        .await
        .unwrap_err();
    assert_eq!(failure, AgentError::Storage(StorageError::IdentityMismatch));
    let reopened = attached_agent(prior, storage.manager().await)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider,
        original.provider
    );
    reopened.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn an_unrestorable_target_cannot_construct_cold_change_authority_or_reach_native_control() {
    let (agent, prior, storage) = prepared(None).await;
    let mut invalid = (*target(&prior)).clone();
    invalid.normalization_failure = true;
    assert!(ModelChange::between(prior.as_ref(), &invalid).is_err());
    let failure = agent
        .set_model(Arc::new(invalid), close_action())
        .await
        .unwrap_err();
    assert_eq!(failure.stage(), ProviderOperationStage::BeforeEffect);
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let reopened = attached_agent(prior.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(reopened.provider_identity(), prior.identity());
    assert_eq!(
        reopened
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .model_change,
        None
    );
    reopened.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn sdk_only_control_refusals_and_uncertainty_keep_original_actor_and_requested_identity() {
    for before_effect in [true, false] {
        let (agent, prior, _) = prepared(None).await;
        let mut selected = (*target(&prior)).clone();
        if before_effect {
            selected.context = "different-workspace".into();
        } else {
            *prior.backend.fail_model.lock().unwrap() =
                Some(AgentError::Protocol("native model receipt lost".into()));
        }
        let expected_target = selected.identity();
        let actor = ActionContext::new("model-person", "model-window", "model-action").unwrap();
        let failure = agent
            .set_model(Arc::new(selected), actor.clone())
            .await
            .unwrap_err();
        let records: Vec<_> = prior
            .backend
            .audit
            .lock()
            .unwrap()
            .iter()
            .filter_map(|record| match record {
                ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].stage(), ConfigurationControlStage::Requested);
        assert_eq!(
            records[1].stage(),
            if before_effect {
                ConfigurationControlStage::Refused
            } else {
                ConfigurationControlStage::Failed
            }
        );
        for record in records {
            assert_eq!(record.actor(), &actor);
            assert_eq!(record.before(), &prior.identity());
            assert_eq!(
                record.request(),
                &ConfigurationControlRequest::Model(expected_target.clone())
            );
            assert_eq!(record.acknowledged(), None);
        }
        assert_eq!(
            failure.stage() == ProviderOperationStage::BeforeEffect,
            before_effect
        );
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn a_custom_usable_failure_is_not_proof_that_a_permission_change_had_no_effect() {
    let (agent, prior, _) = prepared(None).await;
    *prior.backend.fail_approval.lock().unwrap() = Some(AgentError::Unsupported(
        "reported only after native dispatch".into(),
    ));
    let failure = agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap_err();
    assert_eq!(failure.stage(), ProviderOperationStage::EffectUncertain);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert!(agent
        .enqueue(
            input("cannot-assume-old-mode", 1),
            Submitter::owner(close_action())
        )
        .await
        .is_err());
    let stages: Vec<_> = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.stage()),
            _ => None,
        })
        .collect();
    assert_eq!(
        stages,
        vec![
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Failed
        ]
    );
    agent.close(close_action()).await.unwrap();
}

async fn stale_before_effect_replay(model: bool) {
    let (agent, prior, _) = prepared(None).await;
    let mut unrelated = (*target(&prior)).clone();
    unrelated.context = "other-workspace".into();
    let refused = agent
        .set_model(
            Arc::new(unrelated),
            ActionContext::new("person", "window", "unrelated-refusal").unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.stage(), ProviderOperationStage::BeforeEffect);
    let original_error = refused.error().clone();
    *prior.backend.replayed_failure.lock().unwrap() = Some(refused);
    let actor = ActionContext::new("person", "window", "current-native-operation").unwrap();
    let failure = if model {
        agent
            .set_model(target(&prior), actor.clone())
            .await
            .unwrap_err()
    } else {
        agent
            .set_approval_mode(ApprovalMode::Auto, actor.clone())
            .await
            .unwrap_err()
    };
    assert_eq!(
        failure.error(),
        &original_error,
        "diagnostic is retained without trusting a stale operation's origin proof"
    );
    assert_eq!(failure.stage(), ProviderOperationStage::EffectUncertain);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(
        prior.backend.model_calls.load(Ordering::SeqCst),
        usize::from(model)
    );
    assert_eq!(
        prior.backend.approval_calls.load(Ordering::SeqCst),
        usize::from(!model)
    );
    assert!(agent
        .enqueue(
            input("cannot-reuse-stale-proof", 1),
            Submitter::owner(close_action())
        )
        .await
        .is_err());
    let records: Vec<_> = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        records.last().unwrap().stage(),
        ConfigurationControlStage::Failed
    );
    assert_eq!(records.last().unwrap().actor(), &actor);
    agent.close(close_action()).await.unwrap();
}
#[tokio::test]
async fn model_dispatch_cannot_replay_another_operations_before_effect_proof() {
    stale_before_effect_replay(true).await;
}
#[tokio::test]
async fn permission_dispatch_cannot_replay_another_operations_before_effect_proof() {
    stale_before_effect_replay(false).await;
}

// #922 R1: a native ACK must not escape its owning permit during factory settlement.
#[tokio::test]
async fn post_ack_factory_panic_fences_admission_without_committing_model() {
    let (agent, prior, _) = prepared(None).await;
    *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
    let mut chosen = (*target(&prior)).clone();
    chosen.normalization = Normalization::PanicAdjusted;
    let failure = agent
        .set_model(Arc::new(chosen), close_action())
        .await
        .unwrap_err();
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(agent.provider_identity(), prior.identity());
    assert!(agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .model_change
        .is_none());
    assert!(!prior.backend.audit.lock().unwrap().iter().any(|record| matches!(record,
        ExecutionAuditRecord::ConfigurationControlled(record) if record.stage() == ConfigurationControlStage::Applied)));
    assert!(
        agent
            .enqueue(
                input("after-post-ack-panic", 1),
                Submitter::owner(close_action())
            )
            .await
            .is_err(),
        "post-ACK panic must fence subsequent admission"
    );
    supervision::assert_model_failed(&prior);
    agent.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ignored_acknowledged_settings_fence_without_committing_model() {
    for normalization in [Normalization::IgnoreMode, Normalization::IgnoreEffort] {
        let (agent, prior, _) = prepared(None).await;
        let mut chosen = (*target(&prior)).clone();
        chosen.normalization = normalization;
        if matches!(normalization, Normalization::IgnoreMode) {
            *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
        } else {
            *prior.backend.model_effort.lock().unwrap() =
                Some(EffortLevel::new("low".into()).unwrap());
        }
        let outcome = agent.set_model(Arc::new(chosen), close_action()).await;
        assert!(outcome.is_err(), "factory ignored native ACK settings");
        assert_eq!(
            outcome.unwrap_err().session_state(),
            &ProviderSessionState::CleanupRequired
        );
        assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(agent.provider_identity(), prior.identity());
        assert!(agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .model_change
            .is_none());
        assert!(agent
            .enqueue(
                input("after-ignored-ack", 1),
                Submitter::owner(close_action())
            )
            .await
            .is_err());
        agent.close(close_action()).await.unwrap();
    }
}

#[tokio::test]
async fn unsupported_acknowledged_target_preset_fences_without_committing_model() {
    let (agent, prior, _) = prepared(None).await;
    let mut chosen = (*target(&prior)).clone();
    chosen.normalization = Normalization::AskOnly;
    *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
    let failure = agent
        .set_model(Arc::new(chosen), close_action())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error(),
        &AgentError::Unsupported("target accepts Ask only".into())
    );
    assert_eq!(failure.stage(), ProviderOperationStage::EffectUncertain);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(agent.provider_identity(), prior.identity());
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert!(snapshot.model_change.is_none());
    assert!(snapshot.current_configuration.is_none());
    assert_eq!(snapshot.provider, prior.identity());
    assert!(matches!(
        agent
            .enqueue(
                input("after-unsupported-target-ack", 1),
                Submitter::owner(close_action())
            )
            .await,
        Err(AgentError::Closed)
    ));
    let controls = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        controls
            .iter()
            .map(|record| record.stage())
            .collect::<Vec<_>>(),
        [
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Failed
        ]
    );
    assert!(controls
        .iter()
        .all(|record| record.actor() == &close_action() && record.acknowledged().is_none()));
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 0);
    agent.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn adjusted_acknowledgements_install_and_restore_complete_configuration() {
    let (agent, prior, storage) = prepared(None).await;
    *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
    let low = EffortLevel::new("low".into()).unwrap();
    let high = EffortLevel::new("high".into()).unwrap();
    *prior.backend.model_effort.lock().unwrap() = Some(low.clone());
    let chosen = target(&prior);
    let ack = agent
        .set_model(chosen.clone(), close_action())
        .await
        .unwrap();
    assert_eq!(ack.approval_mode(), agent.approval_mode());
    assert_eq!(ack.effort_level(), agent.effort_level().as_ref());
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(agent.effort_level(), Some(low));
    agent
        .set_approval_mode(ApprovalMode::Ask, close_action())
        .await
        .unwrap();
    agent
        .set_effort_level(high.clone(), close_action())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let reopened = attached_agent(chosen, storage.manager().await)
        .await
        .unwrap();
    assert_eq!(reopened.approval_mode(), Some(ApprovalMode::Ask));
    assert_eq!(reopened.effort_level(), Some(high));
    let snapshot = reopened.session_manager().snapshot().await.unwrap();
    assert_eq!(
        snapshot.model_change.as_ref().unwrap().configuration(),
        &ack.configuration()
    );
    assert_eq!(
        snapshot.current_configuration.as_ref().unwrap().kind(),
        SessionConfigurationKind::EffortLevel
    );
    reopened.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn preflight_factory_panic_fences_admission_until_cleanup() {
    let (agent, prior, storage) = prepared(None).await;
    let mut chosen = (*target(&prior)).clone();
    chosen.normalization = Normalization::PanicPreflight;
    let expected_target = chosen.identity();
    let actor =
        ActionContext::new("preflight-person", "preflight-window", "preflight-action").unwrap();
    let failure = agent
        .set_model(Arc::new(chosen), actor.clone())
        .await
        .unwrap_err();
    assert_eq!(failure.error(), &AgentError::SubmissionUnresolved);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(agent.provider_identity(), prior.identity());
    assert!(agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .model_change
        .is_none());
    let controls: Vec<_> = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        controls
            .iter()
            .map(|record| record.stage())
            .collect::<Vec<_>>(),
        [
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Failed
        ]
    );
    assert!(controls.iter().all(|record| record.actor() == &actor
        && record.before() == &prior.identity()
        && record.request() == &ConfigurationControlRequest::Model(expected_target.clone())
        && record.acknowledged().is_none()));
    assert!(
        matches!(
            agent
                .enqueue(
                    input("after-preflight-panic", 1),
                    Submitter::owner(close_action())
                )
                .await,
            Err(AgentError::Closed)
        ),
        "CleanupRequired must refuse subsequent admission even before native entry"
    );
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 0);
    agent.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
    drop(agent);
    let reopened = attached_agent(prior.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(prior.restores.lock().unwrap().len(), 2);
    assert_eq!(reopened.provider_identity(), prior.identity());
    assert_eq!(
        reopened
            .enqueue(
                input("after-preflight-cleanup", 1),
                Submitter::owner(close_action())
            )
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    reopened.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn custom_storage_cannot_restore_model_fact_without_its_current_configuration() {
    let (agent, prior, storage) = prepared(None).await;
    let chosen = target(&prior);
    agent
        .set_model(chosen.clone(), close_action())
        .await
        .unwrap();
    let original = agent.session_manager().snapshot().await.unwrap();
    agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap();
    let later = agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .current_configuration
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    for missing_model in [true, false] {
        let mut malformed = original.clone();
        if missing_model {
            malformed.model_change = None;
        } else {
            malformed.current_configuration = None;
        }
        let units = vec![
            SessionSaveUnit::new(vec![SessionChange::ConfigurationChanged(later.clone())]).unwrap(),
        ];
        assert!(
            matches!(
                SessionSaveUnit::fold(Some(malformed.clone()), &units),
                Err(StorageError::Corrupt(_))
            ),
            "a later valid control cannot hide malformed prior settings"
        );
        storage.0.lock().unwrap().snapshot = Some(malformed);
        let opens = prior.restores.lock().unwrap().len();
        let manager = storage.manager().await;
        let rejected = match Agent::prepare(chosen.clone(), manager, Arc::new(AcceptingAudit)).await
        {
            Err(rejected) => rejected,
            Ok(_) => panic!("malformed custom snapshot restored"),
        };
        assert!(matches!(
            rejected.cause(),
            AgentError::Storage(StorageError::Corrupt(_))
        ));
        assert_eq!(prior.restores.lock().unwrap().len(), opens);
    }
}

#[tokio::test]
async fn dropped_model_caller_keeps_owned_settlement_and_original_actor() {
    let (agent, prior, _) = prepared(None).await;
    let (started, observing) = oneshot::channel();
    let (release, waiting) = oneshot::channel();
    *prior.backend.model_gate.lock().unwrap() = Some((started, waiting));
    let actor = ActionContext::new("person", "window", "disconnected-model-caller").unwrap();
    let chosen = target(&prior);
    let caller = tokio::spawn({
        let agent = agent.clone();
        let chosen = chosen.clone();
        let actor = actor.clone();
        async move { agent.set_model(chosen, actor).await }
    });
    timeout(Duration::from_secs(3), observing)
        .await
        .unwrap()
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    // A later serialized control joins the owning model settlement.
    agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap();
    assert_eq!(agent.provider_identity(), chosen.identity());
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 1);
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.model_change.as_ref().unwrap().actor(), &actor);
    assert!(prior.backend.audit.lock().unwrap().iter().any(|record| matches!(record,
        ExecutionAuditRecord::ConfigurationControlled(record) if record.stage() == ConfigurationControlStage::Applied && record.actor() == &actor)));
    agent.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
}

// #922 R2: same fingerprint does not authorize normalization to replace a host.
#[tokio::test]
async fn normalization_cannot_replace_the_captured_host_owner() {
    for adjusted in [false, true] {
        let (agent, prior, _) =
            prepared_with_normalization(None, Normalization::Exact, Some(Arc::new(UnstartedHost)))
                .await;
        let mut chosen = (*target(&prior)).clone();
        chosen.normalization = if adjusted {
            Normalization::ChangeHostAdjusted
        } else {
            Normalization::ChangeHost
        };
        if adjusted {
            *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
        }
        let failure = agent
            .set_model(Arc::new(chosen), close_action())
            .await
            .unwrap_err();
        assert!(matches!(failure.error(), AgentError::Protocol(_)));
        assert_eq!(
            failure.stage(),
            if adjusted {
                ProviderOperationStage::EffectUncertain
            } else {
                ProviderOperationStage::BeforeEffect
            }
        );
        assert_eq!(
            failure.session_state(),
            if adjusted {
                &ProviderSessionState::CleanupRequired
            } else {
                &ProviderSessionState::Usable
            }
        );
        assert_eq!(
            prior.backend.model_calls.load(Ordering::SeqCst),
            usize::from(adjusted)
        );
        assert_eq!(agent.provider_identity(), prior.identity());
        assert!(agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .model_change
            .is_none());
        assert!(!prior.backend.audit.lock().unwrap().iter().any(|record| matches!(record,
            ExecutionAuditRecord::ConfigurationControlled(record) if record.stage() == ConfigurationControlStage::Applied)));
        let admission = agent
            .enqueue(
                input("after-host-normalization", 1),
                Submitter::owner(close_action()),
            )
            .await;
        if adjusted {
            assert!(matches!(admission, Err(AgentError::Closed)));
        } else {
            assert_eq!(
                admission.unwrap().wait().await,
                Ok(ExecutionOutcome::Completed)
            );
        }
        agent.close(close_action()).await.unwrap();
        assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
    }
    for approval in [true, false] {
        let (agent, prior, _) = prepared_with_normalization(
            None,
            Normalization::ChangeHost,
            Some(Arc::new(UnstartedHost)),
        )
        .await;
        let outcome = if approval {
            agent
                .set_approval_mode(ApprovalMode::Auto, close_action())
                .await
        } else {
            agent
                .set_effort_level(EffortLevel::new("high".into()).unwrap(), close_action())
                .await
        };
        let failure = outcome.unwrap_err();
        assert!(matches!(failure.error(), AgentError::Protocol(_)));
        assert_eq!(failure.stage(), ProviderOperationStage::BeforeEffect);
        assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
        assert_eq!(prior.backend.approval_calls.load(Ordering::SeqCst), 0);
        assert_eq!(prior.backend.effort_calls.load(Ordering::SeqCst), 0);
        assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
        assert_eq!(agent.effort_level(), None);
        assert_eq!(
            agent
                .enqueue(
                    input("after-settings-host-refusal", 1),
                    Submitter::owner(close_action())
                )
                .await
                .unwrap()
                .wait()
                .await,
            Ok(ExecutionOutcome::Completed)
        );
        agent.close(close_action()).await.unwrap();
    }
    let (agent, prior, storage) =
        prepared_with_normalization(None, Normalization::Exact, Some(Arc::new(UnstartedHost)))
            .await;
    agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let opens = prior.restores.lock().unwrap().len();
    let mut changing = (*prior).clone();
    changing.normalization = Normalization::ChangeHost;
    let failure = match attached_agent(Arc::new(changing), storage.manager().await).await {
        Ok(_) => panic!("restoration accepted a clone with another host"),
        Err(failure) => failure,
    };
    assert!(matches!(failure, AgentError::Protocol(_)));
    assert_eq!(prior.restores.lock().unwrap().len(), opens);
    let reopened = attached_agent(prior.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(reopened.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(prior.restores.lock().unwrap().len(), opens + 1);
    reopened.close(close_action()).await.unwrap();
}

#[derive(Clone, Copy)]
enum QueuedConfiguration {
    SmallerLimits,
    RemovedImages,
    RelaxedPreset,
    DirectPreset,
    UnchangedPreset,
}
async fn queued_configuration_keeps_admission_authority(case: QueuedConfiguration) {
    let capabilities = match case {
        QueuedConfiguration::RemovedImages => capabilities_with_images(
            "large",
            1000,
            Some(ImageInputLimitsDto {
                media_types: vec!["image/png".into()],
                max_encoded_bytes: 8,
                max_edge_px: 8000,
                many_images_max_edge_px: 2000,
                native_long_edge_px: 1568,
            }),
        ),
        _ => capabilities_for("large", 1000),
    };
    let (release, hold) = oneshot::channel();
    let (agent, prior, _) =
        prepared_with_capabilities(Some(hold), Normalization::Exact, None, capabilities).await;
    let active = agent
        .enqueue(input("active", 1), Submitter::owner(close_action()))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), prior.backend.started.notified())
        .await
        .unwrap();
    let active_id = agent.active_execution_id();
    let waiting_input = |id: &str| {
        let mut request = input(
            id,
            if matches!(case, QueuedConfiguration::SmallerLimits) {
                400
            } else {
                1
            },
        );
        if matches!(case, QueuedConfiguration::RemovedImages) {
            request.user_message = UserMessage::new(
                request.user_message.text().cloned(),
                vec![ImageReference::new(
                    Sha256Digest::from_bytes([1; 32]),
                    ImageMediaType::Png,
                    1,
                )
                .unwrap()],
                Vec::new(),
            )
            .unwrap();
        }
        request
    };
    let checked = || {
        Submitter::new(
            ActionContext::new("driver", "phone", "queued").unwrap(),
            Initiation::checked_grant(GrantInitiator::new("queued-driver", 1).unwrap()),
        )
    };
    let submitter = || {
        if matches!(
            case,
            QueuedConfiguration::RelaxedPreset
                | QueuedConfiguration::DirectPreset
                | QueuedConfiguration::UnchangedPreset
        ) {
            checked()
        } else {
            Submitter::owner(close_action())
        }
    };
    let queued = agent
        .enqueue(waiting_input("queued"), submitter())
        .await
        .unwrap();
    let boundary = agent
        .enqueue_steering(waiting_input("boundary"), submitter())
        .await
        .unwrap();
    let steered = match agent
        .steer(waiting_input("steered"), checked())
        .await
        .unwrap()
    {
        SteeringDelivery::Queued(receipt) => receipt,
        SteeringDelivery::Injected { .. } => panic!("a distinct grant must wait for its own turn"),
    };
    let mut chosen = (*target(&prior)).clone();
    if matches!(case, QueuedConfiguration::RelaxedPreset) {
        chosen.mode = ApprovalMode::Auto;
        *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
    }
    let control = match case {
        QueuedConfiguration::DirectPreset => {
            agent
                .set_approval_mode(ApprovalMode::Full, close_action())
                .await
        }
        QueuedConfiguration::UnchangedPreset => {
            agent
                .set_approval_mode(ApprovalMode::Ask, close_action())
                .await
        }
        _ => agent
            .set_model(Arc::new(chosen), close_action())
            .await
            .map(|_| ()),
    };
    assert_eq!(agent.active_execution_id(), active_id);
    release.send(()).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), active.wait())
            .await
            .unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let mut outcomes = Vec::new();
    for receipt in [queued, boundary, steered] {
        let id = receipt.id().clone();
        outcomes.push((
            id,
            timeout(Duration::from_secs(3), receipt.wait())
                .await
                .unwrap(),
        ));
    }
    assert!(
        outcomes
            .iter()
            .all(|(_, result)| *result == Ok(ExecutionOutcome::Completed)),
        "admitted work was invalidated: {outcomes:?}; configuration control: {control:?}; preset: {:?}; native model/preset calls: {}/{}",
        agent.approval_mode(),
        prior.backend.model_calls.load(Ordering::SeqCst),
        prior.backend.approval_calls.load(Ordering::SeqCst),
    );
    if matches!(case, QueuedConfiguration::UnchangedPreset) {
        assert!(
            control.is_ok(),
            "unchanged preset remains supported: {control:?}"
        );
    } else {
        let failure =
            control.expect_err("queued work retains its admitted configuration authority");
        assert_eq!(failure.error(), &AgentError::Busy);
        assert_eq!(failure.stage(), ProviderOperationStage::BeforeEffect);
        assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
    }
    assert_eq!(prior.backend.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        prior.backend.approval_calls.load(Ordering::SeqCst),
        usize::from(matches!(case, QueuedConfiguration::UnchangedPreset)),
    );
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 0);
    assert_eq!(agent.provider_identity(), prior.identity());
    assert_eq!(agent.approval_mode(), Some(ApprovalMode::Ask));
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.provider, prior.identity());
    assert!(snapshot.model_change.is_none());
    let stages = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.stage()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        stages,
        if matches!(case, QueuedConfiguration::UnchangedPreset) {
            vec![
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Acknowledged,
                ConfigurationControlStage::Applied,
            ]
        } else {
            vec![
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Refused,
            ]
        }
    );
    agent.close(close_action()).await.unwrap();
}
#[tokio::test]
async fn queued_limits_keep_the_admitted_configuration_authority() {
    queued_configuration_keeps_admission_authority(QueuedConfiguration::SmallerLimits).await;
}
#[tokio::test]
async fn queued_images_keep_the_admitted_configuration_authority() {
    queued_configuration_keeps_admission_authority(QueuedConfiguration::RemovedImages).await;
}
#[tokio::test]
async fn queued_checked_grants_keep_the_admitted_configuration_authority() {
    queued_configuration_keeps_admission_authority(QueuedConfiguration::RelaxedPreset).await;
}

#[tokio::test]
async fn queued_preset_keeps_the_admitted_configuration_authority() {
    queued_configuration_keeps_admission_authority(QueuedConfiguration::DirectPreset).await;
}

#[tokio::test]
async fn queued_unchanged_preset_keeps_the_admitted_configuration_authority() {
    queued_configuration_keeps_admission_authority(QueuedConfiguration::UnchangedPreset).await;
}
