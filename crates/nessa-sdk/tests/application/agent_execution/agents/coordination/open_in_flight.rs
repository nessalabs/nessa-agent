//! An open the scheduler starts on its own is visible while it runs, before the
//! attachment's cleanup fact is armed.
use super::*;
use crate::application::agent_execution::{
    agents::scheduling::QueueAdmission, providers::ProviderCleanup,
};

/// Opens the first attachment at once and holds every later one at a gate.
/// With `keeps_failed`, a later open fails but keeps what it launched.
struct GatedRecoveryProvider {
    inner: Provider,
    opens: AtomicUsize,
    keeps_failed: bool,
    entered: StateMutex<Option<oneshot::Sender<()>>>,
    release: StateMutex<Option<oneshot::Receiver<()>>>,
}
impl AgentProvider for GatedRecoveryProvider {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        if self.opens.fetch_add(1, Ordering::SeqCst) == 0 {
            return self.inner.open(request);
        }
        let entered = self.entered.lock().unwrap().take();
        let release = self.release.lock().unwrap().take();
        Box::pin(async move {
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            if let Some(release) = release {
                let _ = release.await;
            }
            if self.keeps_failed {
                return Err(ProviderOpenError::with_cleanup(
                    AgentError::Protocol("recovery open failed".into()),
                    Arc::new(UnconfirmedCleanup),
                ));
            }
            self.inner.open(request).await
        })
    }
}

/// A launched process whose stop is never confirmed.
struct UnconfirmedCleanup;
impl ProviderCleanup for UnconfirmedCleanup {
    fn retry_cleanup(&self) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::unconfirmed(AgentError::CleanupUncertain) })
    }
}

/// An attached agent whose provider asked for a restart, with work queued so
/// the scheduler reopens it on its own. Returns once that open is at the gate.
async fn recovery_at_the_gate(keeps_failed: bool) -> (Agent, QueueAdmission, oneshot::Sender<()>) {
    let (entered, opening) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let provider = Arc::new(GatedRecoveryProvider {
        inner: Provider(Arc::new(Backend::default())),
        opens: AtomicUsize::new(0),
        keeps_failed,
        entered: StateMutex::new(Some(entered)),
        release: StateMutex::new(Some(gate)),
    });
    let manager = SessionManager::open(
        None,
        Arc::new(InMemoryStorage::new()),
        std::sync::Arc::new(
            nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
        ),
    )
    .await
    .unwrap();
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    assert!(
        agent.may_hold_provider_resources(),
        "an attached provider holds"
    );

    let control = agent.accept_control().unwrap();
    assert!(agent
        .run_control(control, async {
            Err::<(), _>(ProviderOperationFailure::new(
                AgentError::Protocol("provider requested restart".into()),
                ProviderSessionState::CleanupReported(CleanupReport::confirmed(CloseOutcome {
                    forced: false,
                })),
            ))
        })
        .await
        .is_err());
    assert!(
        !agent.may_hold_provider_resources(),
        "the provider reported its release"
    );

    let receipt = agent.enqueue(input(), actor()).await.unwrap();
    timeout(Duration::from_secs(1), opening)
        .await
        .expect("the scheduler starts a recovery open")
        .unwrap();
    assert_eq!(provider.opens.load(Ordering::SeqCst), 2);
    (agent, receipt, release)
}

#[tokio::test]
async fn an_automatic_recovery_open_holds_before_its_cleanup_is_armed() {
    let (agent, receipt, release) = recovery_at_the_gate(false).await;
    assert!(
        !agent.attachment_cleanup_pending(),
        "the cleanup fact is armed only once the open returns"
    );
    assert!(
        agent.may_hold_provider_resources(),
        "an open the scheduler started holds while it runs"
    );

    release.send(()).unwrap();
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    assert!(agent.may_hold_provider_resources());
    agent.close(actor()).await.unwrap();
    assert!(!agent.may_hold_provider_resources());
}

#[tokio::test]
async fn a_failed_recovery_open_that_kept_a_process_still_holds() {
    let (agent, receipt, release) = recovery_at_the_gate(true).await;
    assert!(agent.may_hold_provider_resources());

    release.send(()).unwrap();
    assert!(receipt.wait().await.is_err());
    assert!(
        agent.may_hold_provider_resources(),
        "what the failed open kept is armed before it stops counting as running"
    );
    assert!(agent.attachment_cleanup_pending());
}
