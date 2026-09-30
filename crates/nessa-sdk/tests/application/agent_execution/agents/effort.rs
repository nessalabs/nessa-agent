//! `Agent::set_effort_level` refuses before anything reaches a custom
//! provider backend: while what the agent offers is not negotiated, and for a
//! level it does not offer.

use super::MemoryStorage;
use crate::application::agent_execution::support::*;
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

/// Reports negotiation only once told to, offering `low`; counts levels sent.
struct Backend {
    negotiated: AtomicBool,
    sent: Mutex<Vec<String>>,
    capabilities: EffectiveCapabilities,
    inner: RecordingSession,
}
impl ProviderSessionBackend for Backend {
    fn set_effort_level(&self, level: EffortLevel) -> ProviderOperationFuture<'_, ()> {
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
