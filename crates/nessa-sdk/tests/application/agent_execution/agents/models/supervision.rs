//! One existing control owner supervises every callback through terminal audit.
use super::*;

struct HostilePayload(Arc<AtomicUsize>);
impl Drop for HostilePayload {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("hostile panic payload destructor");
    }
}
pub(super) fn hostile_payload(drops: Arc<AtomicUsize>) -> ! {
    std::panic::panic_any(HostilePayload(drops))
}
fn records(prior: &Provider) -> Vec<ConfigurationControlRecord> {
    prior
        .backend
        .audit
        .lock()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ExecutionAuditRecord::ConfigurationControlled(record) => Some(record.clone()),
            _ => None,
        })
        .collect()
}
pub(super) fn assert_model_failed(prior: &Provider) {
    let records = records(prior);
    assert_eq!(
        records.iter().map(|r| r.stage()).collect::<Vec<_>>(),
        [
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Failed
        ]
    );
    assert!(records.iter().all(|r| r.actor() == &close_action()
        && r.before() == &prior.identity()
        && r.acknowledged().is_none()));
}
async fn check(model: bool, panics: bool, audit_fails: bool, hostile: bool) {
    let (agent, prior, _) = prepared_with_normalization(
        None,
        if !model && panics {
            Normalization::PanicRetained
        } else {
            Normalization::Exact
        },
        None,
    )
    .await;
    prior
        .backend
        .fail_terminal_audit
        .store(audit_fails, Ordering::SeqCst);
    *prior.backend.model_mode.lock().unwrap() = ApprovalMode::Auto;
    let result = if model {
        let mut chosen = (*target(&prior)).clone();
        if panics {
            chosen.normalization = if hostile {
                Normalization::PanicHostileAdjusted
            } else {
                Normalization::PanicAdjusted
            };
        }
        agent
            .set_model(Arc::new(chosen), close_action())
            .await
            .map(|_| ())
    } else {
        agent
            .set_approval_mode(ApprovalMode::Auto, close_action())
            .await
    };
    assert_eq!(
        prior.backend.model_calls.load(Ordering::SeqCst),
        usize::from(model)
    );
    assert_eq!(
        prior.backend.approval_calls.load(Ordering::SeqCst),
        usize::from(!model)
    );
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    if panics {
        let failure = result.unwrap_err();
        assert_eq!(
            failure.session_state(),
            &ProviderSessionState::CleanupRequired
        );
        let expected = if audit_fails {
            AgentError::MultipleOperationFailures {
                first_error: Box::new(AgentError::SubmissionUnresolved),
                subsequent_error: Box::new(AgentError::AuditFailure),
            }
        } else {
            AgentError::SubmissionUnresolved
        };
        assert_eq!(failure.error(), &expected);
        assert_eq!(agent.provider_identity(), prior.identity());
        assert!(matches!(
            agent
                .enqueue(
                    input("after-supervised-panic", 1),
                    Submitter::owner(close_action())
                )
                .await,
            Err(AgentError::Closed)
        ));
    } else {
        result.unwrap();
        assert_eq!(agent.approval_mode(), Some(ApprovalMode::Auto));
    }
    if model && panics {
        assert!(snapshot.model_change.is_none());
        assert!(snapshot.current_configuration.is_none());
    } else {
        let configuration = snapshot.current_configuration.as_ref().unwrap();
        assert_eq!(
            configuration.after().approval_mode(),
            Some(ApprovalMode::Auto)
        );
        assert_eq!(configuration.actor(), &close_action());
        assert_eq!(snapshot.model_change.is_some(), model);
    }
    agent.close(close_action()).await.unwrap();
    assert_eq!(prior.backend.closes.load(Ordering::SeqCst), 1);
    let records = records(&prior);
    let expected = if model && panics {
        vec![
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Failed,
        ]
    } else {
        vec![
            ConfigurationControlStage::Requested,
            ConfigurationControlStage::Acknowledged,
            if panics {
                ConfigurationControlStage::Failed
            } else {
                ConfigurationControlStage::Applied
            },
        ]
    };
    assert_eq!(
        records.iter().map(|r| r.stage()).collect::<Vec<_>>(),
        expected
    );
    assert!(records
        .iter()
        .all(|r| r.actor() == &close_action() && r.before() == &prior.identity()));
    for record in &records {
        if record.stage() == ConfigurationControlStage::Acknowledged {
            let (_, configuration) = record.acknowledged().unwrap();
            assert_eq!(configuration.approval_mode(), Some(ApprovalMode::Auto));
        } else if record.stage() != ConfigurationControlStage::Applied {
            assert!(record.acknowledged().is_none());
        }
    }
    if hostile {
        assert_eq!(
            prior.payload_drops.load(Ordering::SeqCst),
            0,
            "panic payload must not be destroyed"
        );
    }
}
#[tokio::test]
async fn shared_supervision_before_model_panic() {
    check(true, true, false, false).await;
}
#[tokio::test]
async fn shared_supervision_before_approval_panic() {
    check(false, true, false, false).await;
}
#[tokio::test]
async fn shared_supervision_before_model_positive() {
    check(true, false, false, false).await;
}
#[tokio::test]
async fn shared_supervision_before_approval_positive() {
    check(false, false, false, false).await;
}
#[tokio::test]
async fn shared_supervision_model_panic_audit_failure() {
    check(true, true, true, false).await;
}
#[tokio::test]
async fn shared_supervision_approval_panic_audit_failure() {
    check(false, true, true, false).await;
}
#[tokio::test]
async fn shared_supervision_hostile_payload() {
    check(true, true, false, true).await;
}
