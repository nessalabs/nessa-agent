//! `Agent::set_effort_level` refuses before anything reaches a custom
//! provider backend: while what the agent offers is not negotiated, and for a
//! level it does not offer.

use super::MemoryStorage;
use crate::application::agent_execution::support::*;
use nessa_sdk::application::agent_execution::executions::EffortChangeStage;
use nessa_sdk::application::dto::ReasoningDto;
use nessa_sdk::domain::model_metadata::value_objects::{EffortLevel, EffortLevels};
use std::sync::atomic::AtomicBool;

fn level(name: &str) -> EffortLevel {
    EffortLevel::new(name.into()).unwrap()
}

/// A model recording `low` and `high`, through a binding that runs reasoning.
fn reasoning_capabilities() -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: text,
        image_input: None,
        output: text,
        tool_use: true,
        reasoning: Some(ReasoningDto {
            effort_levels: vec!["low".into(), "high".into()],
        }),
        fast_mode: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(
            ModelFeatures::new(text, text, true, true, false),
            model.limits(),
        ),
        model.limits(),
    )
    .unwrap()
}

/// Reports negotiation only once told to, offering `low`; counts levels sent,
/// or refuses them as busy once told to.
struct Backend {
    negotiated: AtomicBool,
    busy: AtomicBool,
    sent: Mutex<Vec<String>>,
    capabilities: EffectiveCapabilities,
    inner: RecordingSession,
}
impl ProviderSessionBackend for Backend {
    fn set_effort_level(&self, level: EffortLevel) -> ProviderOperationFuture<'_, ()> {
        if self.busy.load(Ordering::SeqCst) {
            return Box::pin(async {
                Err(ProviderOperationFailure::new(
                    AgentError::Busy,
                    ProviderSessionState::Usable,
                ))
            });
        }
        self.sent.lock().unwrap().push(level.as_str().to_owned());
        Box::pin(async { Ok(()) })
    }
    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        let levels: &EffortLevels = self.capabilities.effort_levels().unwrap();
        ProviderOperationCapabilities {
            negotiated: self.negotiated.load(Ordering::SeqCst),
            effort_levels: levels.offered_by(["low"]),
            ..ProviderOperationCapabilities::default()
        }
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

struct Provider(Arc<Backend>);
impl AgentProvider for Provider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("effort-fixture", "fixture", "test").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.0.capabilities
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    ExecutionSessionId::new("context").unwrap(),
                    self.0.clone(),
                    self.0.capabilities.clone(),
                ),
                events: Box::new(Events),
            })
        })
    }
}

#[tokio::test]
async fn nothing_is_sent_until_negotiated_nor_a_level_not_offered() {
    let backend = Arc::new(Backend {
        negotiated: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        sent: Mutex::new(Vec::new()),
        capabilities: reasoning_capabilities(),
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
    });
    let agent = attached_agent(
        Arc::new(Provider(backend.clone())),
        MemoryStorage::default().manager().await,
    )
    .await
    .unwrap();
    // Attached, but not negotiated (a restore in progress): not yet, not "not offered".
    let failure = agent
        .set_effort_level(level("low"), close_action())
        .await
        .unwrap_err();
    assert_eq!(
        failure.error(),
        &AgentError::AttachmentUnavailable(AttachmentPhase::Starting)
    );
    assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
    assert_eq!(agent.effort_levels(), None);

    backend.negotiated.store(true, Ordering::SeqCst);
    // A catalogue level the agent does not offer.
    let failure = agent
        .set_effort_level(level("high"), close_action())
        .await
        .unwrap_err();
    assert!(matches!(failure.error(), AgentError::InvalidInput(_)));
    assert!(backend.sent.lock().unwrap().is_empty());

    agent
        .set_effort_level(level("low"), close_action())
        .await
        .unwrap();
    assert_eq!(*backend.sent.lock().unwrap(), ["low"]);
    assert_eq!(agent.effort_level(), Some(level("low")));
    agent.close(close_action()).await.unwrap();
}

/// Keeps the stage of each effort change record.
#[derive(Default)]
struct Stages(Mutex<Vec<EffortChangeStage>>);
impl ExecutionAudit for Stages {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if let ExecutionAuditRecord::EffortLevelChanged(change) = record {
            self.0.lock().unwrap().push(change.stage());
        }
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_change_the_connection_refuses_unsent_is_recorded_as_refused() {
    let backend = Arc::new(Backend {
        negotiated: AtomicBool::new(true),
        busy: AtomicBool::new(true),
        sent: Mutex::new(Vec::new()),
        capabilities: reasoning_capabilities(),
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
    });
    let stages = Arc::new(Stages::default());
    let agent = Agent::prepare(
        Arc::new(Provider(backend.clone())),
        MemoryStorage::default().manager().await,
        stages.clone(),
    )
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
    let failure = agent
        .set_effort_level(level("low"), close_action())
        .await
        .unwrap_err();
    assert_eq!(failure.error(), &AgentError::Busy);
    assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
    assert_eq!(
        *stages.0.lock().unwrap(),
        [EffortChangeStage::Requested, EffortChangeStage::Refused]
    );
    assert_eq!(agent.effort_level(), None);
    agent.close(close_action()).await.unwrap();
}

/// Holds each effort change record until released, to drop its caller mid-change.
struct Gated {
    stages: Mutex<Vec<EffortChangeStage>>,
    gate: tokio::sync::Semaphore,
}
impl ExecutionAudit for Gated {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let stage = match record {
            ExecutionAuditRecord::EffortLevelChanged(change) => Some(change.stage()),
            _ => None,
        };
        Box::pin(async move {
            if let Some(stage) = stage {
                self.stages.lock().unwrap().push(stage);
                if stage == EffortChangeStage::Requested {
                    self.gate.acquire().await.unwrap().forget();
                }
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_change_whose_caller_goes_away_still_settles_and_is_recorded() {
    let backend = Arc::new(Backend {
        negotiated: AtomicBool::new(true),
        busy: AtomicBool::new(false),
        sent: Mutex::new(Vec::new()),
        capabilities: reasoning_capabilities(),
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
    });
    let audit = Arc::new(Gated {
        stages: Mutex::new(Vec::new()),
        gate: tokio::sync::Semaphore::new(0),
    });
    let agent = Agent::prepare(
        Arc::new(Provider(backend.clone())),
        MemoryStorage::default().manager().await,
        audit.clone(),
    )
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
    // The caller gives up while the request is being recorded.
    let waited = tokio::time::timeout(
        std::time::Duration::from_millis(20),
        agent.set_effort_level(level("low"), close_action()),
    )
    .await;
    assert!(waited.is_err());
    assert_eq!(
        *audit.stages.lock().unwrap(),
        [EffortChangeStage::Requested]
    );
    audit.gate.add_permits(1);
    for _ in 0..100 {
        if audit.stages.lock().unwrap().len() == 2 {
            break;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    assert_eq!(
        *audit.stages.lock().unwrap(),
        [EffortChangeStage::Requested, EffortChangeStage::Applied]
    );
    assert_eq!(*backend.sent.lock().unwrap(), ["low"]);
    assert_eq!(agent.effort_level(), Some(level("low")));
    agent.close(close_action()).await.unwrap();
}

/// Panics on the settlement of an effort change.
struct Panicking;
impl ExecutionAudit for Panicking {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        if let ExecutionAuditRecord::EffortLevelChanged(change) = &record {
            if change.stage() != EffortChangeStage::Requested {
                panic!("sink failed");
            }
        }
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_sink_that_panics_on_the_settlement_has_not_recorded_it() {
    let backend = Arc::new(Backend {
        negotiated: AtomicBool::new(true),
        busy: AtomicBool::new(false),
        sent: Mutex::new(Vec::new()),
        capabilities: reasoning_capabilities(),
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
    });
    let agent = Agent::prepare(
        Arc::new(Provider(backend.clone())),
        MemoryStorage::default().manager().await,
        Arc::new(Panicking),
    )
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
    let failure = agent
        .set_effort_level(level("low"), close_action())
        .await
        .unwrap_err();
    // Applied, and not recorded: in force, and no turn may run under it.
    assert_eq!(failure.error(), &AgentError::AuditFailure);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(agent.effort_level(), Some(level("low")));
    let _ = agent.close(close_action()).await;
}

/// Panics while its future is polled, on the settlement of an effort change.
struct PanickingLater;
impl ExecutionAudit for PanickingLater {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let settles = matches!(
            &record,
            ExecutionAuditRecord::EffortLevelChanged(change)
                if change.stage() != EffortChangeStage::Requested
        );
        Box::pin(async move {
            if settles {
                panic!("sink failed while delivering");
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn a_sink_that_panics_delivering_the_settlement_has_not_recorded_it() {
    let backend = Arc::new(Backend {
        negotiated: AtomicBool::new(true),
        busy: AtomicBool::new(false),
        sent: Mutex::new(Vec::new()),
        capabilities: reasoning_capabilities(),
        inner: RecordingSession {
            prompts: AtomicUsize::new(0),
        },
    });
    let agent = Agent::prepare(
        Arc::new(Provider(backend.clone())),
        MemoryStorage::default().manager().await,
        Arc::new(PanickingLater),
    )
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
    let failure = agent
        .set_effort_level(level("low"), close_action())
        .await
        .unwrap_err();
    assert_eq!(failure.error(), &AgentError::AuditFailure);
    assert_eq!(
        failure.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    let _ = agent.close(close_action()).await;
}
