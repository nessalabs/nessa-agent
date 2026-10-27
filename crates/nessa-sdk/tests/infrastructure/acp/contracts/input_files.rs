//! File preparation spends the native operation's deadline before any handoff.
use super::support::*;
use crate::domain::common::value_objects::{MediaType, Sha256Digest};
use crate::infrastructure::acp::executions::steering::RESPONSE_TIMEOUT;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{mpsc, Semaphore};

struct GatedFiles {
    began: mpsc::UnboundedSender<()>,
    release: Semaphore,
}
impl InputFileResolver for GatedFiles {
    fn resolve(&self, _: ExecutionId, files: Vec<UploadedFileReference>) -> InputFileFuture<'_> {
        Box::pin(async move {
            self.began.send(()).unwrap();
            self.release.acquire().await.unwrap().forget();
            Ok(files
                .into_iter()
                .map(|file| {
                    PreparedInputFile::new(
                        file,
                        LinkedFile::new("/workspace/notes.txt".into()).unwrap(),
                    )
                })
                .collect())
        })
    }
}
fn file_request() -> ExecutionRequest {
    ExecutionRequest {
        user_message: UserMessage::new(
            Some(PromptText::new("look").unwrap()),
            vec![],
            vec![],
            vec![UploadedFileReference::new(
                Sha256Digest::from_bytes([9; 32]),
                MediaType::parse("text/plain").unwrap(),
                4,
                "notes.txt".into(),
            )
            .unwrap()],
        )
        .unwrap(),
        ..prompt("file")
    }
}
async fn opened_files(
    mode: &str,
    limit: Option<Duration>,
) -> (
    TempDir,
    OpenedProviderSession,
    Arc<ManualClock>,
    Arc<GatedFiles>,
    mpsc::UnboundedReceiver<()>,
) {
    let (root, mut config, model) = test_acp_configuration(mode, 16);
    config.execution_timeout = limit;
    config.max_frame_bytes = 128 * 1024;
    let clock = manual_clock(&mut config);
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    let mut opened = provider
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let (began, begins) = mpsc::unbounded_channel();
    let source = Arc::new(GatedFiles {
        began,
        release: Semaphore::new(0),
    });
    opened.session = opened.session.with_input_files(source.clone());
    (root, opened, clock, source, begins)
}
async fn close(opened: &OpenedProviderSession, clock: &ManualClock) {
    promptly(clock.passing(a_grace, async {
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap();
    }))
    .await;
}
#[tokio::test]
async fn pending_or_ready_file_preparation_cannot_dispatch_at_execution_expiry() {
    let _slot = process_test_slot().await;
    for ready in [false, true] {
        let limit = Duration::from_secs(4);
        let (root, opened, clock, source, mut begins) =
            opened_files("image-input", Some(limit)).await;
        let session = opened.session.clone();
        let mut running = tokio::spawn(async move {
            session
                .execute(file_request(), Arc::new(|| panic!("late handoff")))
                .await
        });
        tokio::select! {
            began = begins.recv() => assert_eq!(began, Some(())),
            answer = &mut running => panic!("returned before preparation: {answer:?}"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
        }
        clock.advance(limit);
        if ready {
            source.release.add_permits(1);
        }
        assert!(matches!(
            promptly(clock.passing(a_grace, running)).await.unwrap(),
            ProviderExecutionReply::Rejected(AgentError::Deadline)
        ));
        assert!(!root.path().join("prompt-observed").exists());
        close(&opened, &clock).await;
    }
}
#[tokio::test]
async fn pending_or_ready_file_preparation_cannot_dispatch_at_steering_expiry() {
    let _slot = process_test_slot().await;
    for ready in [false, true] {
        let (root, mut opened, clock, source, mut begins) =
            opened_files("steering-image-injected", None).await;
        let active = start(&opened, "first").await;
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Message(MessageChunk::text("running:first"))
        );
        let session = opened.session.clone();
        let mut running = tokio::spawn(async move {
            session
                .steer(
                    ExecutionId::new("first").unwrap(),
                    file_request(),
                    &session.capabilities(),
                    &|| {},
                )
                .await
        });
        tokio::select! {
            began = begins.recv() => assert_eq!(began, Some(())),
            answer = &mut running => panic!("returned before preparation: {answer:?}"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
        }
        clock.advance(RESPONSE_TIMEOUT);
        if ready {
            source.release.add_permits(1);
        }
        let error = promptly(clock.passing(a_grace, running))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.session_state(), &ProviderSessionState::Usable);
        assert_eq!(error.into_error(), AgentError::Deadline);
        assert!(!root.path().join("steering-observed").exists());
        close(&opened, &clock).await;
        let _ = promptly(clock.passing(a_grace, active)).await;
    }
}
#[tokio::test]
async fn file_preparation_spends_the_execution_reply_budget() {
    let _slot = process_test_slot().await;
    let limit = Duration::from_secs(4);
    let (_root, mut opened, clock, source, mut begins) =
        opened_files("image-stall", Some(limit)).await;
    let session = opened.session.clone();
    let mut running = tokio::spawn(async move {
        session
            .execute(file_request(), Arc::new(|| Ok(())))
            .await
            .into_result()
    });
    tokio::select! {
        began = begins.recv() => assert_eq!(began, Some(())),
        answer = &mut running => panic!("returned before preparation: {answer:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
    }
    clock.advance(limit / 2);
    source.release.add_permits(1);
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running"))
    );
    clock.advance(limit / 2);
    assert_eq!(
        promptly(clock.passing(a_grace, running)).await.unwrap(),
        Err(AgentError::Deadline)
    );
}
#[tokio::test]
async fn file_preparation_spends_the_steering_reply_budget() {
    let _slot = process_test_slot().await;
    let (root, mut opened, clock, source, mut begins) =
        opened_files("steering-image-stall", None).await;
    let active = start(&opened, "first").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:first"))
    );
    let session = opened.session.clone();
    let mut running = tokio::spawn(async move {
        session
            .steer(
                ExecutionId::new("first").unwrap(),
                file_request(),
                &session.capabilities(),
                &|| {},
            )
            .await
            .map_err(|error| error.into_error())
    });
    tokio::select! {
        began = begins.recv() => assert_eq!(began, Some(())),
        answer = &mut running => panic!("returned before preparation: {answer:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
    }
    clock.advance(RESPONSE_TIMEOUT / 2);
    source.release.add_permits(1);
    wait_for_file(&root, "steering-observed").await;
    clock.advance(RESPONSE_TIMEOUT - RESPONSE_TIMEOUT / 2);
    assert_eq!(
        promptly(clock.passing(a_grace, running)).await.unwrap(),
        Err(AgentError::Deadline)
    );
    let _ = promptly(clock.passing(a_grace, active)).await;
}
#[tokio::test]
async fn close_during_file_preparation_rejects_before_handoff() {
    let _slot = process_test_slot().await;
    let (root, opened, clock, _source, mut begins) = opened_files("image-input", None).await;
    let session = opened.session.clone();
    let mut running = tokio::spawn(async move {
        session
            .execute(file_request(), Arc::new(|| panic!("closed handoff")))
            .await
    });
    tokio::select! {
        began = begins.recv() => assert_eq!(began, Some(())),
        answer = &mut running => panic!("returned before preparation: {answer:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
    }
    close(&opened, &clock).await;
    assert!(matches!(
        promptly(clock.passing(a_grace, running)).await.unwrap(),
        ProviderExecutionReply::Rejected(AgentError::Closed)
    ));
    assert!(!root.path().join("prompt-observed").exists());
}
#[tokio::test]
async fn file_preparation_before_the_deadline_dispatches_the_confirmed_link() {
    let _slot = process_test_slot().await;
    let (root, opened, clock, source, mut begins) =
        opened_files("image-input", Some(Duration::from_secs(4))).await;
    let session = opened.session.clone();
    let mut running = tokio::spawn(async move {
        session
            .execute(file_request(), Arc::new(|| Ok(())))
            .await
            .into_result()
    });
    tokio::select! {
        began = begins.recv() => assert_eq!(began, Some(())),
        answer = &mut running => panic!("returned before preparation: {answer:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
    }
    source.release.add_permits(1);
    assert_eq!(
        promptly(clock.passing(a_grace, running)).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let observed = std::fs::read_to_string(root.path().join("prompt-observed")).unwrap();
    assert!(observed.contains("/workspace/notes.txt"));
    close(&opened, &clock).await;
}

#[tokio::test]
async fn close_during_steering_file_preparation_releases_the_handoff_without_dispatch() {
    let _slot = process_test_slot().await;
    let (root, mut opened, clock, _source, mut begins) =
        opened_files("steering-image-injected", None).await;
    let active = start(&opened, "first").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:first"))
    );
    let session = opened.session.clone();
    let started = Arc::new(AtomicBool::new(false));
    let observed = started.clone();
    let mut running = tokio::spawn(async move {
        session
            .steer(
                ExecutionId::new("first").unwrap(),
                file_request(),
                &session.capabilities(),
                &|| {
                    observed.store(true, Ordering::SeqCst);
                },
            )
            .await
    });
    tokio::select! {
        began = begins.recv() => assert_eq!(began, Some(())),
        answer = &mut running => panic!("returned before preparation: {answer:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("preparation did not begin"),
    }
    close(&opened, &clock).await;
    let error = promptly(clock.passing(a_grace, running))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.session_state(),
        &ProviderSessionState::CleanupRequired
    );
    assert_eq!(error.into_error(), AgentError::Closed);
    assert!(started.load(Ordering::SeqCst));
    assert!(!root.path().join("steering-observed").exists());
    let _ = promptly(clock.passing(a_grace, active)).await;
}
