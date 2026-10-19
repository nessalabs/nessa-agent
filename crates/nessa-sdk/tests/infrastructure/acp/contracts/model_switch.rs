//! Both pinned ACP profile contracts retain an already running native turn.
use super::support::*;
use crate::application::agent_execution::providers::{ModelChange, ModelChangeTiming};
use crate::application::agent_execution::sessions::SessionManager;
use crate::application::dto::ModelMetadataDto;
use crate::domain::model_metadata::value_objects::EffortLevel;
use crate::infrastructure::session_storage::InMemoryStorage;
use crate::infrastructure::session_storage::RuntimeMessageCommitClock;

async fn switching(claude: bool, adjustment: bool, hosted: bool) {
    let _slot = process_test_slot().await;
    let (root, mut config, mut model) = if claude {
        test_acp_configuration("model-switch", 64)
    } else {
        codex_configuration("model-switch", 64)
    };
    config.arguments = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/model_switch.py")
            .into_os_string(),
        if claude {
            "claude".into()
        } else {
            "codex".into()
        },
    ];
    if adjustment {
        let mut initial = ModelMetadataDto::from(&model);
        initial.model_id = if claude {
            "claude-sonnet-5"
        } else {
            "gpt-6-astra"
        }
        .into();
        initial.reasoning = Some(ReasoningDto {
            effort_levels: vec!["medium".into(), "high".into()],
        });
        model = ModelMetadata::try_from(initial).unwrap();
        config.arguments.push("adjust".into());
    }
    let mut metadata = ModelMetadataDto::from(&model);
    metadata.model_id = if adjustment {
        if claude {
            "claude-haiku-4-5-20251001"
        } else {
            "gpt-5.6-terra"
        }
    } else {
        "selected-model"
    }
    .into();
    if adjustment {
        metadata.reasoning = if claude {
            None
        } else {
            Some(ReasoningDto {
                effort_levels: vec!["medium".into()],
            })
        };
    }
    metadata.max_context_window_tokens = 700;
    let selected = ModelMetadata::try_from(metadata).unwrap();
    let limits = TokenLimits::new(600, 100).unwrap();
    let audit = Arc::new(RecordingAudit::default());
    let prior: Arc<dyn AgentProvider> = if claude {
        Arc::new(ClaudeAcpProvider::new(config.clone(), &model, limits, audit.clone()).unwrap())
    } else {
        Arc::new(CodexAcpProvider::new(config.clone(), &model, limits, audit.clone()).unwrap())
    };
    let target: Arc<dyn AgentProvider> = if claude {
        Arc::new(ClaudeAcpProvider::new(config.clone(), &selected, limits, audit.clone()).unwrap())
    } else {
        Arc::new(CodexAcpProvider::new(config, &selected, limits, audit.clone()).unwrap())
    };
    let host = Arc::new(ModelHost {
        workspace: std::fs::canonicalize(root.path()).unwrap(),
        claude,
        adjustment,
        starts: std::sync::atomic::AtomicUsize::new(0),
        cleanups: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let foreign = Arc::new(UnstartedHost {
        workspace: host.workspace.clone(),
        starts: std::sync::atomic::AtomicUsize::new(0),
    });
    let (prior, target) = if hosted {
        (
            prior.on_host(host.clone()).unwrap(),
            target.on_host(host.clone()).unwrap(),
        )
    } else {
        (prior, target)
    };
    let prior: Arc<dyn AgentProvider> = if adjustment {
        prior
            .with_model_configuration(
                Some(ApprovalMode::Auto),
                Some(EffortLevel::new("high".into()).unwrap()),
                prior.traces(),
            )
            .unwrap()
    } else {
        prior
    };
    let prior_model = prior.identity().model_id().to_owned();
    let selected_model = target.identity().model_id().to_owned();
    let storage = Arc::new(InMemoryStorage::default());
    let manager = SessionManager::open(
        None,
        storage.clone(),
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let agent = Agent::prepare(prior, manager, audit.clone()).await.unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    let mut events = agent.subscribe();
    let input = |id: &str| ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::text_only(PromptText::new(id.to_owned()).unwrap()),
        estimated_input_tokens: 1,
        reserved_output_tokens: 100,
    };
    let first = agent
        .enqueue(input("before-model"), Submitter::owner(close_action()))
        .await
        .unwrap();
    timeout(Duration::from_secs(3), async { loop { let event = events.next().await.unwrap().unwrap(); if matches!(event.update(), ExecutionUpdate::Message(text) if text.as_str() == "native turn running") { break; } } }).await.unwrap();
    let active = agent.active_execution_id();
    let context = agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .provider_context;
    if hosted {
        let failure = agent
            .set_model(target.on_host(foreign.clone()).unwrap(), close_action())
            .await
            .unwrap_err();
        assert!(matches!(failure.error(), AgentError::Configuration(_)));
        assert_eq!(failure.stage(), ProviderOperationStage::BeforeEffect);
        assert_eq!(failure.session_state(), &ProviderSessionState::Usable);
        assert_eq!(host.starts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(foreign.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(agent.active_execution_id(), active);
        assert!(agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .model_change
            .is_none());
        let records = audit.configuration.lock().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.stage())
                .collect::<Vec<_>>(),
            vec![
                ConfigurationControlStage::Requested,
                ConfigurationControlStage::Refused
            ]
        );
        assert!(records
            .iter()
            .all(|record| record.actor() == &close_action() && record.acknowledged().is_none()));
    }
    let effect = timeout(
        Duration::from_secs(3),
        agent.set_model(target.clone(), close_action()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(effect.identity(), &target.identity());
    assert_eq!(
        effect.timing(),
        if claude {
            ModelChangeTiming::SubsequentResponses
        } else {
            ModelChangeTiming::NextTurn
        }
    );
    assert!(active.is_some());
    let controls = audit.configuration.lock().unwrap().clone();
    let controls = &controls[if hosted { 2 } else { 0 }..];
    assert_eq!(
        controls
            .iter()
            .map(|record| record.stage())
            .collect::<Vec<_>>(),
        vec![
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Acknowledged,
            ConfigurationControlStage::Applied
        ]
    );
    assert_eq!(
        controls[1].acknowledged(),
        Some((effect.identity(), &effect.configuration()))
    );
    assert_eq!(controls[0].before().model_id(), prior_model);
    assert_eq!(
        controls[0].request(),
        &ConfigurationControlRequest::Model(target.identity())
    );
    if adjustment {
        assert_eq!(effect.approval_mode(), Some(ApprovalMode::Ask));
        assert_eq!(
            effect.effort_level().map(|level| level.as_str()),
            if claude { None } else { Some("medium") }
        );
        assert_eq!(agent.effort_level().as_ref(), effect.effort_level());
    }
    assert_eq!(
        timeout(Duration::from_secs(3), first.wait()).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .provider_context,
        context
    );
    assert_eq!(
        agent
            .enqueue(input("after-model"), Submitter::owner(close_action()))
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    let traces = agent.read_traces(agent.session_manager().id().as_str());
    let models: Vec<_> = traces
        .iter()
        .filter_map(|span| span.string("gen_ai.request.model"))
        .collect();
    assert!(models.contains(&prior_model.as_str()), "{models:?}");
    assert!(models.contains(&selected_model.as_str()), "{models:?}");
    assert_eq!(
        agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations
            .len(),
        2
    );
    let session_id = agent.session_manager().id().clone();
    timeout(Duration::from_secs(3), agent.close(close_action()))
        .await
        .unwrap()
        .unwrap();
    drop(agent);
    if hosted {
        assert_eq!(host.cleanups.load(std::sync::atomic::Ordering::SeqCst), 1);
        let manager = SessionManager::open(
            Some(session_id),
            storage,
            Arc::new(RuntimeMessageCommitClock::new()),
        )
        .await
        .unwrap();
        let reopened = attached_agent(target, manager).await.unwrap();
        assert_eq!(
            reopened
                .session_manager()
                .snapshot()
                .await
                .unwrap()
                .provider_context,
            context
        );
        assert_eq!(host.starts.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(foreign.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
        reopened.close(close_action()).await.unwrap();
        assert_eq!(host.cleanups.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
    drop(root);
}
#[tokio::test]
async fn claude_model_switch_keeps_native_turn_and_both_model_traces() {
    switching(true, false, false).await;
}
#[tokio::test]
async fn codex_model_switch_keeps_native_turn_and_both_model_traces() {
    switching(false, false, false).await;
}

#[test]
fn claude_model_change_proof_preserves_its_native_launch_output_ceiling() {
    let (_root, config, model) = test_acp_configuration("model-switch", 64);
    let mut metadata = ModelMetadataDto::from(&model);
    metadata.model_id = "other-model".into();
    let target = ModelMetadata::try_from(metadata).unwrap();
    let audit = Arc::new(RecordingAudit::default());
    let prior = ClaudeAcpProvider::new(
        config.clone(),
        &model,
        TokenLimits::new(900, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    let same = ClaudeAcpProvider::new(
        config.clone(),
        &target,
        TokenLimits::new(800, 100).unwrap(),
        audit.clone(),
    )
    .unwrap();
    assert!(ModelChange::between(&prior, &same).is_ok());
    let changed =
        ClaudeAcpProvider::new(config, &target, TokenLimits::new(800, 150).unwrap(), audit)
            .unwrap();
    assert!(ModelChange::between(&prior, &changed).is_err());
}

#[tokio::test]
async fn claude_model_ack_can_drop_auto_and_effort_together() {
    switching(true, true, false).await;
}
#[tokio::test]
async fn codex_model_ack_can_change_effort_without_rewriting_active_trace() {
    switching(false, true, false).await;
}

struct UnstartedHost {
    workspace: PathBuf,
    starts: std::sync::atomic::AtomicUsize,
}
impl crate::application::agent_execution::providers::HarnessHost for UnstartedHost {
    fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }
    fn start(
        &self,
        _launch: crate::application::agent_execution::providers::HarnessLaunch,
    ) -> Result<crate::application::agent_execution::providers::HarnessProcess, AgentError> {
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(AgentError::Closed)
    }
}

// #922 R2: equal workspace/configuration attributes do not identify a host owner.
#[test]
fn model_choices_require_the_same_captured_host_owner() {
    for claude in [true, false] {
        let (root, config, model) = if claude {
            test_acp_configuration("model-switch", 64)
        } else {
            codex_configuration("model-switch", 64)
        };
        let workspace = std::fs::canonicalize(root.path()).unwrap();
        let config = config.on_host(workspace.clone());
        let mut selected = ModelMetadataDto::from(&model);
        selected.model_id = "selected-model".into();
        let selected = ModelMetadata::try_from(selected).unwrap();
        let audit = Arc::new(RecordingAudit::default());
        let limits = TokenLimits::new(600, 100).unwrap();
        let make = |model: &ModelMetadata| -> Arc<dyn AgentProvider> {
            if claude {
                Arc::new(
                    ClaudeAcpProvider::new(config.clone(), model, limits, audit.clone()).unwrap(),
                )
            } else {
                Arc::new(
                    CodexAcpProvider::new(config.clone(), model, limits, audit.clone()).unwrap(),
                )
            }
        };
        let a = Arc::new(UnstartedHost {
            workspace: workspace.clone(),
            starts: std::sync::atomic::AtomicUsize::new(0),
        });
        let b = Arc::new(UnstartedHost {
            workspace,
            starts: std::sync::atomic::AtomicUsize::new(0),
        });
        let local_prior = make(&model);
        let local_target = make(&selected);
        let hosted_prior = local_prior.on_host(a.clone()).unwrap();
        let shared_target = local_target.on_host(a.clone()).unwrap();
        let foreign_target = local_target.on_host(b.clone()).unwrap();
        assert!(ModelChange::between(local_prior.as_ref(), local_target.as_ref()).is_ok());
        assert!(ModelChange::between(hosted_prior.as_ref(), shared_target.as_ref()).is_ok());
        for (prior, target) in [
            (&hosted_prior, &foreign_target),
            (&local_prior, &shared_target),
            (&hosted_prior, &local_target),
        ] {
            assert_eq!(prior.model_change_context(), target.model_change_context());
            assert!(
                matches!(
                    ModelChange::between(prior.as_ref(), target.as_ref()),
                    Err(AgentError::Configuration(_))
                ),
                "matching configuration must not authorize another live host"
            );
        }
        assert_eq!(a.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(b.starts.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

struct ModelHost {
    workspace: PathBuf,
    claude: bool,
    adjustment: bool,
    starts: std::sync::atomic::AtomicUsize,
    cleanups: Arc<std::sync::atomic::AtomicUsize>,
}
struct ModelHostControl {
    inner: Box<dyn crate::application::agent_execution::providers::HarnessControl>,
    cleanups: Arc<std::sync::atomic::AtomicUsize>,
}
impl crate::application::agent_execution::providers::HarnessControl for ModelHostControl {
    fn cleanup(
        &mut self,
        grace: Duration,
        kill_timeout: Duration,
    ) -> crate::application::agent_execution::providers::HarnessCleanupFuture<'_> {
        self.cleanups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.cleanup(grace, kill_timeout)
    }
}
impl crate::application::agent_execution::providers::HarnessHost for ModelHost {
    fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }
    fn start(
        &self,
        launch: crate::application::agent_execution::providers::HarnessLaunch,
    ) -> Result<crate::application::agent_execution::providers::HarnessProcess, AgentError> {
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut command = tokio::process::Command::new("/usr/bin/python3");
        command
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/infrastructure/acp/contracts/fixtures/model_switch.py"),
            )
            .arg(if self.claude { "claude" } else { "codex" })
            .current_dir(&self.workspace)
            .env_clear()
            .envs(launch.environment);
        if self.adjustment {
            command.arg("adjust");
        }
        let process = crate::infrastructure::harness_process::SupervisedHarness::start(command)?;
        Ok(
            crate::application::agent_execution::providers::HarnessProcess {
                control: Box::new(ModelHostControl {
                    inner: process.control,
                    cleanups: self.cleanups.clone(),
                }),
                ..process
            },
        )
    }
}
#[tokio::test]
async fn claude_hosted_model_choice_retains_owner_through_normalization_and_restore() {
    switching(true, true, true).await;
}
#[tokio::test]
async fn codex_hosted_model_choice_retains_owner_through_normalization_and_restore() {
    switching(false, true, true).await;
}
