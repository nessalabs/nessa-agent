//! Retirement storage faults settle through the existing writer and lifecycle owners.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewRetirementPanic {
    Construction,
    PollBeforeCommit,
    PollAfterCommit,
    DropBeforeCommit,
    DropAfterCommit,
}
#[derive(Clone, Copy, Debug)]
enum ReviewAttachmentAction {
    Abandon,
    AuthorizedClose,
    Start,
}

async fn review_retirement_panic_probe(
    mode: ReviewRetirementPanic,
    action: ReviewAttachmentAction,
) {
    use nessa_sdk::application::agent_execution::{sessions::LeaseRecord, subagents::*};
    use nessa_sdk::domain::agent_execution::{leases::*, subagents::*};
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    };

    struct NoFactory;
    #[async_trait::async_trait]
    impl ChildFactory for NoFactory {
        async fn prepare(&self, _: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
            panic!("root-only review probe does not prepare a child")
        }
    }
    struct Accept;
    #[async_trait::async_trait]
    impl OwnershipAudit for Accept {
        async fn record(&self, _: &OwnershipEvidence) -> Result<(), PortFailure> {
            Ok(())
        }
    }
    struct PanickingStorage {
        backing: MemoryStorage,
        mode: ReviewRetirementPanic,
        armed: Arc<AtomicBool>,
        reached: Arc<tokio::sync::Notify>,
    }
    struct PanickingLease {
        backing: Box<dyn SessionStorageLease>,
        mode: ReviewRetirementPanic,
        armed: Arc<AtomicBool>,
        reached: Arc<tokio::sync::Notify>,
    }
    struct PanickingSave<'a> {
        backing: StorageFuture<'a, SessionSaveReceipt>,
        mode: ReviewRetirementPanic,
    }
    impl Future for PanickingSave<'_> {
        type Output = Result<SessionSaveReceipt, StorageError>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let this = self.get_mut();
            match this.mode {
                ReviewRetirementPanic::PollBeforeCommit => {
                    panic!("review retirement poll before commit")
                }
                ReviewRetirementPanic::DropBeforeCommit => Poll::Ready(Err(StorageError::Io(
                    "review retirement rejected before drop panic".into(),
                ))),
                ReviewRetirementPanic::PollAfterCommit => match this.backing.as_mut().poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(result) => {
                        assert!(result.is_ok(), "the before-panic storage operation commits");
                        panic!("review retirement poll after commit")
                    }
                },
                ReviewRetirementPanic::DropAfterCommit => this.backing.as_mut().poll(cx),
                ReviewRetirementPanic::Construction => {
                    unreachable!("construction panic returns no future")
                }
            }
        }
    }
    impl Drop for PanickingSave<'_> {
        fn drop(&mut self) {
            if matches!(
                self.mode,
                ReviewRetirementPanic::DropBeforeCommit | ReviewRetirementPanic::DropAfterCommit
            ) {
                panic!("review retirement future drop");
            }
        }
    }
    impl SessionStorage for PanickingStorage {
        fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
            Box::pin(async move {
                Ok(Box::new(PanickingLease {
                    backing: self.backing.open(id).await?,
                    mode: self.mode,
                    armed: self.armed.clone(),
                    reached: self.reached.clone(),
                }) as Box<dyn SessionStorageLease>)
            })
        }
    }
    impl SessionStorageLease for PanickingLease {
        fn load(&self) -> StorageFuture<'_, SessionLoad> {
            self.backing.load()
        }
        fn save_changes(
            &self,
            binding: SessionSaveGeneration,
            units: Vec<SessionSaveUnit>,
        ) -> StorageFuture<'_, SessionSaveReceipt> {
            let retires = units
                .iter()
                .flat_map(SessionSaveUnit::changes)
                .any(|change| {
                    matches!(
                        change,
                        SessionChange::Lease(LeaseRecord::SdkCleanupRetired { .. })
                    )
                });
            let inject = retires && self.armed.swap(false, Ordering::SeqCst);
            if inject {
                self.reached.notify_one();
                if self.mode == ReviewRetirementPanic::Construction {
                    panic!("review retirement future construction");
                }
            }
            let backing = self.backing.save_changes(binding, units);
            if inject {
                Box::pin(PanickingSave {
                    backing,
                    mode: self.mode,
                })
            } else {
                backing
            }
        }
        fn erase(&self) -> StorageFuture<'_, ()> {
            self.backing.erase()
        }
    }

    let storage = MemoryStorage::default();
    let armed = Arc::new(AtomicBool::new(false));
    let reached = Arc::new(tokio::sync::Notify::new());
    let manager = SessionManager::open(
        Some(SessionId::new("conversation").unwrap()),
        Arc::new(PanickingStorage {
            backing: storage.clone(),
            mode,
            armed: armed.clone(),
            reached: reached.clone(),
        }),
        Arc::new(nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let backend = workflow_backend();
    let agent = Agent::prepare(
        Arc::new(WorkflowProvider(backend.clone())),
        manager,
        backend.audit.clone(),
    )
    .await
    .unwrap();
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(MemoryOwnershipStore::new()),
        audit: Arc::new(Accept),
        factory: Arc::new(NoFactory),
        room: Arc::new(LiveCapacity::new(1)),
    });
    let root = coordinator
        .open_root(agent.session_manager().id().clone(), Initiator::Runtime)
        .await
        .unwrap();
    let gate = coordinator.participation(&root).unwrap();
    let scope = gate.cleanup_scope().unwrap();
    agent.install_owned_lifetime(gate).unwrap();
    agent
        .session_manager()
        .record_lease(|_| {
            (
                vec![LeaseRecord::Issued {
                    lease: LeaseId::new("review-retirement-panic").unwrap(),
                    revision: LeaseRevision::FIRST,
                    terms: LeaseTerms {
                        environment: EnvironmentRef::Here,
                        work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                        sandbox: SandboxProfile::HarnessDefault,
                        grants: LeaseGrants::Opening,
                        deadline: LeaseDeadline::UntilEnded,
                    },
                    actor: actor(),
                }],
                (),
            )
        })
        .await
        .unwrap()
        .saved
        .unwrap();
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    bounded(agent.start_attachment(authorization).unwrap().wait())
        .await
        .unwrap();
    bounded(agent.close_report(actor()))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert!(storage
        .snapshot()
        .lease
        .as_ref()
        .unwrap()
        .sdk_cleanup_acknowledged(&scope));
    let before_audit = backend.audit.records.lock().unwrap().len();
    armed.store(true, Ordering::SeqCst);
    let mut authorization = Some(
        agent
            .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
            .unwrap(),
    );
    let mut starting = None;
    let mut closing = None;
    match action {
        ReviewAttachmentAction::Abandon => drop(authorization.take()),
        ReviewAttachmentAction::AuthorizedClose => {
            closing = Some(tokio::spawn({
                let agent = agent.clone();
                async move { agent.close_report(actor()).await }
            }));
        }
        ReviewAttachmentAction::Start => {
            starting = Some(
                agent
                    .start_attachment(authorization.take().unwrap())
                    .unwrap(),
            )
        }
    }
    bounded(reached.notified()).await;
    assert_eq!(
        backend.audit.records.lock().unwrap().len(),
        before_audit,
        "retirement fault precedes any new attachment audit"
    );
    let committed = storage
        .snapshot()
        .lease
        .as_ref()
        .unwrap()
        .sdk_cleanup_acknowledged(&scope);
    assert_eq!(
        committed,
        !matches!(
            mode,
            ReviewRetirementPanic::PollAfterCommit | ReviewRetirementPanic::DropAfterCommit
        ),
        "the fixture distinguishes before-commit failure from committed retirement"
    );
    if let Some(starting) = starting {
        assert!(
            bounded(starting.wait()).await.is_err(),
            "faulted start cannot succeed"
        );
    }
    if let Some(closing) = closing {
        let report = bounded(closing).await.unwrap().unwrap();
        assert!(report.cleanup().is_confirmed());
        assert!(
            report.cleanup().audit().is_err(),
            "real close retains its mandatory evidence failure"
        );
    } else {
        let terminal = timeout(Duration::from_secs(2), async {
            while matches!(
                agent.attachment_status().phase(),
                AttachmentPhase::Waiting | AttachmentPhase::Starting
            ) {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(terminal.is_ok(), "retirement port panic must settle the attachment; mode={mode:?}, action={action:?}, status={:?}", agent.attachment_status());
    }
    assert!(agent.attachment_status().evidence_failure().is_some(), "retirement panic retains an attributed attachment evidence failure; mode={mode:?}, action={action:?}");
    assert!(!agent.may_hold_provider_resources());
    let report = bounded(agent.close_report(actor())).await.unwrap();
    assert!(report.cleanup().is_confirmed());
    assert!(report.cleanup().audit().is_err());
    drop(authorization);
}

#[tokio::test]
async fn cycle10_retirement_panic_abandon_construction() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::Construction,
        ReviewAttachmentAction::Abandon,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_abandon_poll_before_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::PollBeforeCommit,
        ReviewAttachmentAction::Abandon,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_abandon_poll_after_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::PollAfterCommit,
        ReviewAttachmentAction::Abandon,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_abandon_drop_before_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::DropBeforeCommit,
        ReviewAttachmentAction::Abandon,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_abandon_drop_after_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::DropAfterCommit,
        ReviewAttachmentAction::Abandon,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_authorized_close_construction() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::Construction,
        ReviewAttachmentAction::AuthorizedClose,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_authorized_close_poll_before_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::PollBeforeCommit,
        ReviewAttachmentAction::AuthorizedClose,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_authorized_close_poll_after_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::PollAfterCommit,
        ReviewAttachmentAction::AuthorizedClose,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_authorized_close_drop_before_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::DropBeforeCommit,
        ReviewAttachmentAction::AuthorizedClose,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_authorized_close_drop_after_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::DropAfterCommit,
        ReviewAttachmentAction::AuthorizedClose,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_start_construction() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::Construction,
        ReviewAttachmentAction::Start,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_start_poll_before_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::PollBeforeCommit,
        ReviewAttachmentAction::Start,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_start_poll_after_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::PollAfterCommit,
        ReviewAttachmentAction::Start,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_start_drop_before_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::DropBeforeCommit,
        ReviewAttachmentAction::Start,
    )
    .await;
}

#[tokio::test]
async fn cycle10_retirement_panic_start_drop_after_commit() {
    review_retirement_panic_probe(
        ReviewRetirementPanic::DropAfterCommit,
        ReviewAttachmentAction::Start,
    )
    .await;
}
