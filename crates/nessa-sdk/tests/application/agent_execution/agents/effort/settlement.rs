//! Native acknowledgement and local configuration settlement keep distinct evidence.
use super::*;
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};

#[derive(Clone, Copy)]
enum Fault {
    None,
    Storage,
    Retain,
    RetainPanic,
    Native,
    NativePanic,
}
struct Native {
    inner: Backend,
    fault: Fault,
    closes: AtomicUsize,
}
impl ProviderSessionBackend for Native {
    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        self.inner.operation_capabilities()
    }
    fn set_effort_level(&self, effort: EffortLevel) -> ProviderOperationFuture<'_, ()> {
        let result = self.inner.set_effort_level(effort);
        Box::pin(async move {
            result.await?;
            match self.fault {
                Fault::Native => Err(ProviderOperationFailure::new(
                    AgentError::Protocol("native unverified".into()),
                    ProviderSessionState::CleanupRequired,
                )),
                Fault::NativePanic => panic!("native control panic"),
                _ => Ok(()),
            }
        })
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        self.inner.prepare_invocation()
    }
    fn execute(
        &self,
        input: ExecutionRequest,
        handoff: NativeExecutionHandoff,
    ) -> ProviderExecutionFuture<'_> {
        self.inner.execute(input, handoff)
    }
    fn answer_question(&self, input: QuestionAnswer) -> ProviderOperationFuture<'_, ()> {
        self.inner.answer_question(input)
    }
    fn answer_permission(
        &self,
        input: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        self.inner.answer_permission(input)
    }
    fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        self.inner.cancel_permission(input)
    }
    fn close(&self, input: SessionCloseRequest) -> CleanupFuture<'_> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.inner.close(input)
    }
}
#[derive(Clone)]
struct Factory {
    native: Arc<Native>,
    effort: Option<EffortLevel>,
    clones: Arc<AtomicUsize>,
}
impl AgentProvider for Factory {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("effort-settlement", "fixture", "original").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.native.inner.capabilities
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.effort.clone()
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        _: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        assert_eq!(mode, self.approval_mode());
        if self.clones.fetch_add(1, Ordering::SeqCst) == 1 {
            match self.native.fault {
                Fault::Retain => return Err(AgentError::Protocol("retention refused".into())),
                Fault::RetainPanic => panic!("retention factory panic"),
                _ => {}
            }
        }
        Ok(Arc::new(Self {
            effort,
            ..self.clone()
        }))
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let (_, restore, _) = request.into_parts();
        Box::pin(async move {
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    restore.unwrap_or_else(|| ExecutionSessionId::new("native-effort").unwrap()),
                    self.native.clone(),
                    self.capabilities().clone(),
                ),
                events: Box::new(Events),
            })
        })
    }
}
struct Store {
    inner: InMemoryStorage,
    fail: Arc<AtomicBool>,
}
struct Lease {
    inner: Box<dyn SessionStorageLease>,
    fail: Arc<AtomicBool>,
}
impl SessionStorage for Store {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        Box::pin(async move {
            Ok(Box::new(Lease {
                inner: self.inner.open(id).await?,
                fail: self.fail.clone(),
            }) as Box<dyn SessionStorageLease>)
        })
    }
}
impl SessionStorageLease for Lease {
    fn load(&self) -> StorageFuture<'_, SessionLoad> {
        self.inner.load()
    }
    fn erase(&self) -> StorageFuture<'_, ()> {
        self.inner.erase()
    }
    fn save_changes(
        &self,
        binding: SessionSaveGeneration,
        units: Vec<SessionSaveUnit>,
    ) -> StorageFuture<'_, SessionSaveReceipt> {
        if units
            .iter()
            .flat_map(SessionSaveUnit::changes)
            .any(|c| matches!(c, SessionChange::ConfigurationChanged(_)))
            && self.fail.swap(false, Ordering::SeqCst)
        {
            return Box::pin(async { Err(StorageError::Io("configuration save refused".into())) });
        }
        self.inner.save_changes(binding, units)
    }
}
struct Audit {
    attempts: Mutex<Vec<EffortLevelChangeRecord>>,
    fail_terminal: bool,
}
impl ExecutionAudit for Audit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        let fail = if let ExecutionAuditRecord::EffortLevelChanged(change) = record {
            let terminal = change.stage() != EffortChangeStage::Requested;
            self.attempts.lock().unwrap().push(change);
            terminal && self.fail_terminal
        } else {
            false
        };
        Box::pin(async move {
            if fail {
                Err(AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}
async fn check(fault: Fault, audit_fails: bool) {
    let native = Arc::new(Native {
        inner: Backend {
            negotiated: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            sent: Mutex::new(Vec::new()),
            capabilities: reasoning_capabilities(),
            inner: RecordingSession {
                prompts: AtomicUsize::new(0),
            },
        },
        fault,
        closes: AtomicUsize::new(0),
    });
    let manager = SessionManager::open(
        None,
        Arc::new(Store {
            inner: InMemoryStorage::new(),
            fail: Arc::new(AtomicBool::new(matches!(fault, Fault::Storage))),
        }),
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    let audit = Arc::new(Audit {
        attempts: Mutex::new(Vec::new()),
        fail_terminal: audit_fails,
    });
    let agent = Agent::prepare(
        Arc::new(Factory {
            native: native.clone(),
            effort: None,
            clones: Arc::new(AtomicUsize::new(0)),
        }),
        manager,
        audit.clone(),
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
    let result = agent.set_effort_level(level("low"), close_action()).await;
    assert_eq!(*native.inner.sent.lock().unwrap(), vec!["low"]);
    let snapshot = agent.session_manager().snapshot().await.unwrap();
    let acknowledged = !matches!(fault, Fault::Native | Fault::NativePanic);
    let failed = !matches!(fault, Fault::None) || audit_fails;
    if failed {
        let failure = result.unwrap_err();
        assert_eq!(
            failure.session_state(),
            &ProviderSessionState::CleanupRequired
        );
        let expected = match fault {
            Fault::Storage => {
                AgentError::Storage(StorageError::Io("configuration save refused".into()))
            }
            Fault::Retain => AgentError::Protocol("retention refused".into()),
            Fault::RetainPanic => {
                AgentError::Protocol("effort configuration retention panicked".into())
            }
            Fault::Native => AgentError::Protocol("native unverified".into()),
            Fault::NativePanic => AgentError::Protocol("provider control panicked".into()),
            Fault::None => AgentError::AuditFailure,
        };
        if audit_fails && !matches!(fault, Fault::None) {
            assert_eq!(
                failure.error(),
                &AgentError::MultipleOperationFailures {
                    first_error: Box::new(expected),
                    subsequent_error: Box::new(AgentError::AuditFailure)
                }
            );
        } else {
            assert_eq!(failure.error(), &expected);
        }
        assert!(matches!(
            agent
                .enqueue(
                    ExecutionRequest {
                        execution_id: ExecutionId::new("after-effort-failure").unwrap(),
                        user_message: UserMessage::text_only(PromptText::new("later").unwrap()),
                        estimated_input_tokens: 1,
                        reserved_output_tokens: 1
                    },
                    Submitter::owner(close_action())
                )
                .await,
            Err(AgentError::Closed)
        ));
    } else {
        result.unwrap();
        assert_eq!(agent.effort_level(), Some(level("low")));
    }
    if matches!(fault, Fault::None | Fault::Retain | Fault::RetainPanic) {
        let saved = snapshot.current_configuration.unwrap();
        assert_eq!(saved.after().effort_level(), Some(&level("low")));
        assert_eq!(saved.actor(), &close_action());
    } else {
        assert!(snapshot.current_configuration.is_none());
    }
    agent.close(close_action()).await.unwrap();
    assert_eq!(native.closes.load(Ordering::SeqCst), 1);
    let attempts = audit.attempts.lock().unwrap();
    assert_eq!(
        attempts.iter().map(|r| r.stage()).collect::<Vec<_>>(),
        [
            EffortChangeStage::Requested,
            if acknowledged {
                EffortChangeStage::Applied
            } else {
                EffortChangeStage::Failed
            }
        ]
    );
    assert!(attempts.iter().all(|r| r.actor() == &close_action()
        && r.before().is_none()
        && r.after() == &level("low")));
    assert_eq!(attempts[0].session_id(), attempts[1].session_id());
    assert_eq!(
        attempts[0].attachment_generation(),
        attempts[1].attachment_generation()
    );
}
#[tokio::test]
async fn effort_settlement_storage_failure_keeps_ack() {
    check(Fault::Storage, false).await;
}
#[tokio::test]
async fn effort_settlement_storage_success() {
    check(Fault::None, false).await;
}
#[tokio::test]
async fn effort_settlement_retention_failure_keeps_ack() {
    check(Fault::Retain, false).await;
}
#[tokio::test]
async fn effort_settlement_retention_panic_keeps_ack() {
    check(Fault::RetainPanic, false).await;
}
#[tokio::test]
async fn effort_settlement_storage_and_audit_preserve_primary() {
    check(Fault::Storage, true).await;
}
#[tokio::test]
async fn effort_settlement_retention_and_audit_preserve_primary() {
    check(Fault::Retain, true).await;
}
#[tokio::test]
async fn effort_settlement_unverified_native_is_failed() {
    check(Fault::Native, false).await;
}
#[tokio::test]
async fn effort_settlement_native_panic_is_failed() {
    check(Fault::NativePanic, false).await;
}
