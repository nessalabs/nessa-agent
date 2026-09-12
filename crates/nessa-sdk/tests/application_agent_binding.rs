use nessa_sdk::{
    application::{
        agent_binding::*,
        dto::{ModalitiesDto, ModelMetadataDto},
    },
    domain::{
        common::value_objects::TokenLimits,
        effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
        model_metadata::{
            entities::ModelMetadata,
            value_objects::{Modalities, ModelFeatures},
        },
    },
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct RecordingSession {
    prompts: AtomicUsize,
}
impl AgentSession for RecordingSession {
    fn prompt(&self, _input: Prompt) -> BindingFuture<'_, PromptOutcome> {
        Box::pin(async move {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            Ok(PromptOutcome::Completed)
        })
    }
    fn answer_permission(&self, _answer: PermissionAnswer) -> BindingFuture<'_, ()> {
        Box::pin(async { Err(BindingError::StalePermission) })
    }
    fn stop(&self) -> BindingFuture<'_, StopOutcome> {
        Box::pin(async { Ok(StopOutcome { forced: false }) })
    }
}
struct OfflineSession;
impl AgentSession for OfflineSession {
    fn prompt(&self, _input: Prompt) -> BindingFuture<'_, PromptOutcome> {
        Box::pin(async { Err(BindingError::Closed) })
    }
    fn answer_permission(&self, _answer: PermissionAnswer) -> BindingFuture<'_, ()> {
        Box::pin(async { Err(BindingError::Closed) })
    }
    fn stop(&self) -> BindingFuture<'_, StopOutcome> {
        Box::pin(async { Err(BindingError::CleanupUncertain) })
    }
}
fn capabilities() -> EffectiveCapabilities {
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
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let text = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(ModelFeatures::new(text, text, true, false), model.limits()),
        model.limits(),
    )
    .unwrap()
}
#[tokio::test]
async fn capability_admission_is_independent_of_provider_or_transport() {
    let session = Arc::new(RecordingSession {
        prompts: AtomicUsize::new(0),
    });
    let agent = Agent::new(session.clone(), capabilities());
    let input = Prompt {
        execution_id: "execution".into(),
        text: "hello".into(),
        input_tokens: 900,
        reserved_output_tokens: 100,
    };
    for rejected in [
        Prompt {
            input_tokens: 901,
            ..input.clone()
        },
        Prompt {
            text: " \n".into(),
            ..input.clone()
        },
        Prompt {
            reserved_output_tokens: 0,
            ..input.clone()
        },
    ] {
        assert!(matches!(
            agent.prompt(rejected).await,
            Err(BindingError::InvalidInput(_))
        ));
    }
    assert_eq!(session.prompts.load(Ordering::SeqCst), 0);
    assert_eq!(agent.prompt(input).await.unwrap(), PromptOutcome::Completed);
    assert_eq!(session.prompts.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.capabilities().limits(),
        TokenLimits::new(1000, 100).unwrap()
    );
}
#[tokio::test]
async fn adapter_substitution_keeps_instances_and_controls_isolated() {
    let recording = Arc::new(RecordingSession {
        prompts: AtomicUsize::new(0),
    });
    let online = Agent::new(recording.clone(), capabilities());
    let offline = Agent::new(Arc::new(OfflineSession), capabilities());
    let input = Prompt {
        execution_id: "execution".into(),
        text: "hello".into(),
        input_tokens: 1,
        reserved_output_tokens: 100,
    };
    assert_eq!(
        offline.prompt(input.clone()).await,
        Err(BindingError::Closed)
    );
    assert_eq!(recording.prompts.load(Ordering::SeqCst), 0);
    assert_eq!(
        online.prompt(input).await.unwrap(),
        PromptOutcome::Completed
    );
    assert_eq!(
        online
            .answer_permission(PermissionAnswer {
                execution_id: "write".into(),
                id: "1".into(),
                allow_once: true
            })
            .await,
        Err(BindingError::StalePermission)
    );
    assert_eq!(
        offline
            .answer_permission(PermissionAnswer {
                execution_id: "write".into(),
                id: "1".into(),
                allow_once: true
            })
            .await,
        Err(BindingError::Closed)
    );
    assert_eq!(online.stop().await.unwrap(), StopOutcome { forced: false });
    assert_eq!(offline.stop().await, Err(BindingError::CleanupUncertain));
}
