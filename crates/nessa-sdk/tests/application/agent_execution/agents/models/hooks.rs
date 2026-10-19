//! A settings clone retains the original factory's independently published hook policy.
use super::*;

async fn model_hook_refusal(adjusted: bool) {
    let (agent, prior, _) = prepared(None).await;
    let mut chosen = (*target(&prior)).clone();
    chosen.normalization = if adjusted {
        Normalization::ChangeHooksAdjusted
    } else {
        Normalization::ChangeHooks
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
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    assert!(snapshot.model_change.is_none());
    assert!(snapshot.current_configuration.is_none());
    let controls = prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            ExecutionAuditRecord::ConfigurationControlled(r) => Some(r.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        controls.iter().map(|r| r.stage()).collect::<Vec<_>>(),
        [
            ConfigurationControlStage::Requested,
            if adjusted {
                ConfigurationControlStage::Failed
            } else {
                ConfigurationControlStage::Refused
            }
        ]
    );
    assert!(controls
        .iter()
        .all(|r| r.actor() == &close_action() && r.acknowledged().is_none()));
    assert!(!controls
        .iter()
        .any(|r| r.stage() == ConfigurationControlStage::Applied));
    let next = agent
        .enqueue(input("hook-refusal", 1), Submitter::owner(close_action()))
        .await;
    if adjusted {
        assert!(matches!(next, Err(AgentError::Closed)));
    } else {
        assert_eq!(next.unwrap().wait().await, Ok(ExecutionOutcome::Completed));
    }
    agent.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
}

async fn settings_hook_refusal(approval: bool) {
    let (agent, prior, _) =
        prepared_with_normalization(None, Normalization::ChangeHooks, None).await;
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
    assert!(agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .current_configuration
        .is_none());
    assert_eq!(
        agent
            .enqueue(
                input("settings-hook-refusal", 1),
                Submitter::new(
                    close_action(),
                    Initiation::checked_grant(GrantInitiator::new("hook-driver", 1).unwrap())
                )
            )
            .await
            .unwrap()
            .wait()
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn hook_normalization_saved_settings_refuse_before_open() {
    let (agent, prior, storage) = prepared(None).await;
    agent
        .set_approval_mode(ApprovalMode::Auto, close_action())
        .await
        .unwrap();
    agent.close(close_action()).await.unwrap();
    drop(agent);
    let opens = prior.restores.lock().unwrap().len();
    let mut changed = (*prior).clone();
    changed.normalization = Normalization::ChangeHooks;
    let failure = match attached_agent(Arc::new(changed), storage.manager().await).await {
        Ok(agent) => {
            agent.close(close_action()).await.unwrap();
            panic!("saved configuration enabled native hooks")
        }
        Err(e) => e,
    };
    assert!(matches!(failure, AgentError::Protocol(_)));
    assert_eq!(prior.restores.lock().unwrap().len(), opens);
    let restored = attached_agent(prior.clone(), storage.manager().await)
        .await
        .unwrap();
    assert_eq!(restored.approval_mode(), Some(ApprovalMode::Auto));
    assert_eq!(*prior.opened_hooks.lock().unwrap(), vec![false, false]);
    restored.close(close_action()).await.unwrap();
}

async fn exact_clone_attachments() {
    for allowed in [false, true] {
        for explicit in [false, true] {
            let (initial, prior, storage) = prepared(None).await;
            initial.close(close_action()).await.unwrap();
            drop(initial);
            let mut configured = (*prior).clone();
            configured.native_hooks = allowed;
            configured.opened_hooks.lock().unwrap().clear();
            let configured = Arc::new(configured);
            let agent = attached_agent(configured.clone(), storage.manager().await)
                .await
                .unwrap();
            agent
                .set_model(target(&configured), close_action())
                .await
                .unwrap();
            if explicit {
                agent.suspend(close_action()).await.unwrap();
            } else {
                *configured.backend.execution_failure.lock().unwrap() =
                    Some(ProviderOperationFailure::new(
                        AgentError::Protocol("native exit".into()),
                        ProviderSessionState::CleanupRequired,
                    ));
                assert_eq!(
                    agent
                        .enqueue(input("native-exit", 1), Submitter::owner(close_action()))
                        .await
                        .unwrap()
                        .wait()
                        .await,
                    Err(AgentError::Protocol("native exit".into()))
                );
                let auth = agent
                    .authorize_attachment(AttachmentRequest::AutomaticRecovery)
                    .unwrap();
                agent.start_attachment(auth).unwrap().wait().await.unwrap();
            }
            if explicit {
                let auth = agent
                    .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
                    .unwrap();
                agent.start_attachment(auth).unwrap().wait().await.unwrap();
            }
            assert_eq!(
                agent
                    .enqueue(
                        input("exact-hook-reattach", 1),
                        Submitter::owner(close_action())
                    )
                    .await
                    .unwrap()
                    .wait()
                    .await,
                Ok(ExecutionOutcome::Completed)
            );
            assert_eq!(
                *configured.opened_hooks.lock().unwrap(),
                vec![allowed, allowed]
            );
            agent.close(close_action()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn hook_normalization_model_preflight_refuses() {
    model_hook_refusal(false).await;
}
#[tokio::test]
async fn hook_normalization_post_ack_fences() {
    model_hook_refusal(true).await;
}
#[tokio::test]
async fn hook_normalization_approval_preflight_refuses() {
    settings_hook_refusal(true).await;
}
#[tokio::test]
async fn hook_normalization_effort_preflight_refuses() {
    settings_hook_refusal(false).await;
}

#[tokio::test]
async fn hook_normalization_exact_clones_keep_explicit_and_automatic_attachments() {
    timeout(Duration::from_secs(3), exact_clone_attachments())
        .await
        .expect("public reattachments settle");
}
