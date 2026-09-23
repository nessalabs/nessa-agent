//! Startup controls remain bound to one owned provider attachment generation.
use super::*;

struct IgnoredStartupControlProvider {
    inner: Arc<TestProvider>,
    entered: Notify,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    observed: Mutex<Vec<Option<SessionCloseRequest>>>,
}

impl AgentProvider for IgnoredStartupControlProvider {
    fn identity(&self) -> ProviderIdentity {
        self.inner.identity()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        self.inner.capabilities()
    }

    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let release = self.release.lock().unwrap().take();
        Box::pin(async move {
            let (restore, control) = request.into_parts();
            self.entered.notify_one();
            if let Some(release) = release {
                let _ = release.await;
            }
            self.observed.lock().unwrap().push(control.requested());
            self.inner
                .open(ProviderOpenRequest::without_startup_control(restore))
                .await
        })
    }
}

#[tokio::test]
async fn ignored_control_stays_owned_and_cannot_stop_the_replacement_generation() {
    let storage = MemoryStorage::default();
    let inner = TestProvider::new();
    let (release, waiting) = oneshot::channel();
    let provider = Arc::new(IgnoredStartupControlProvider {
        inner: inner.clone(),
        entered: Notify::new(),
        release: Mutex::new(Some(waiting)),
        observed: Mutex::new(Vec::new()),
    });
    let agent = prepared(provider.clone(), &storage, Arc::new(AcceptingAudit)).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    provider.entered.notified().await;
    drop(attachment);

    let first_actor = ActionContext::new("owner", "phone", "first-close").unwrap();
    let first = tokio::spawn({
        let agent = agent.clone();
        let first_actor = first_actor.clone();
        async move { agent.close(first_actor).await }
    });
    while agent.attachment_status().phase() != AttachmentPhase::Absent {
        tokio::task::yield_now().await;
    }
    let second_actor = ActionContext::new("owner", "panel", "second-close").unwrap();
    let second = tokio::spawn({
        let agent = agent.clone();
        async move { agent.close(second_actor).await }
    });
    tokio::task::yield_now().await;
    assert!(!first.is_finished());
    assert!(!second.is_finished());

    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        provider.observed.lock().unwrap().as_slice(),
        &[Some(SessionCloseRequest::Explicit(first_actor.clone()))]
    );
    assert_eq!(inner.calls.opens.lock().unwrap().len(), 1);
    assert_eq!(inner.calls.closes.lock().unwrap().len(), 1);

    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert_eq!(
        provider.observed.lock().unwrap().as_slice(),
        &[Some(SessionCloseRequest::Explicit(first_actor)), None]
    );
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Attached);
    agent.close(close_action()).await.unwrap();
}

#[tokio::test]
async fn close_after_provider_ready_waits_for_publication_and_cleans_the_actual_session() {
    let storage = MemoryStorage::default();
    let provider = TestProvider::new();
    let audit = Arc::new(AttachmentAuditProbe::default());
    *audit.gate_after.lock().unwrap() = Some(AttachmentAuditStage::Starting);
    let (release_start, waiting_start) = oneshot::channel();
    *audit.release.lock().unwrap() = Some(waiting_start);
    let agent = prepared(provider.clone(), &storage, audit.clone()).await;
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor()))
        .unwrap();
    let attachment = agent.start_attachment(authorization).unwrap();
    audit.entered.notified().await;

    let (saving_context, release_context) = storage.pause_next_save();
    release_start.send(()).unwrap();
    saving_context.await.unwrap();
    assert_eq!(provider.calls.opens.lock().unwrap().len(), 1);
    assert!(provider.calls.closes.lock().unwrap().is_empty());
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Starting);

    let request = close_action();
    let closing = tokio::spawn({
        let agent = agent.clone();
        let request = request.clone();
        async move { agent.close(request).await }
    });
    while agent.attachment_status().phase() != AttachmentPhase::Absent {
        tokio::task::yield_now().await;
    }
    assert!(!closing.is_finished());
    assert!(provider.calls.closes.lock().unwrap().is_empty());

    release_context.send(()).unwrap();
    assert_eq!(attachment.wait().await, Err(AgentError::Closed));
    closing.await.unwrap().unwrap();
    assert_eq!(
        provider.calls.closes.lock().unwrap().as_slice(),
        &[SessionCloseRequest::Explicit(request)]
    );
    assert_eq!(agent.attachment_status().phase(), AttachmentPhase::Absent);
}
