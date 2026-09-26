//! An open the scheduler starts on its own is visible while it runs, before the
//! attachment's cleanup fact is armed.
use super::*;

/// Opens the first attachment at once and holds every later one at a gate.
struct GatedRecoveryProvider {
    inner: Provider,
    opens: AtomicUsize,
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
            self.inner.open(request).await
        })
    }
}

#[tokio::test]
async fn an_automatic_recovery_open_is_in_flight_before_its_cleanup_is_armed() {
    let (entered, opening) = oneshot::channel();
    let (release, gate) = oneshot::channel();
    let provider = Arc::new(GatedRecoveryProvider {
        inner: Provider(Arc::new(Backend::default())),
        opens: AtomicUsize::new(0),
        entered: StateMutex::new(Some(entered)),
        release: StateMutex::new(Some(gate)),
    });
    let manager = SessionManager::open(None, Arc::new(InMemoryStorage::new()))
        .await
        .unwrap();
    let agent = attached_agent(provider.clone(), manager).await.unwrap();
    assert!(!agent.provider_open_in_flight());

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
    assert!(!agent.attachment_cleanup_pending());

    // Queued work makes the scheduler reopen the provider by itself.
    let receipt = agent.enqueue(input(), actor()).await.unwrap();
    timeout(Duration::from_secs(1), opening)
        .await
        .expect("the scheduler starts a recovery open")
        .unwrap();
    assert_eq!(provider.opens.load(Ordering::SeqCst), 2);
    assert!(
        !agent.attachment_cleanup_pending(),
        "the cleanup fact is armed only once the open returns"
    );
    assert!(
        agent.provider_open_in_flight(),
        "an open the scheduler started is visible while it runs"
    );

    release.send(()).unwrap();
    assert_eq!(receipt.wait().await, Ok(ExecutionOutcome::Completed));
    assert!(!agent.provider_open_in_flight());
    assert!(agent.attachment_cleanup_pending());
    agent.close(actor()).await.unwrap();
    assert!(!agent.attachment_cleanup_pending());
}
