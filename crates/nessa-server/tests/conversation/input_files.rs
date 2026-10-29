use super::*;
use crate::conversation::application::{
    ConversationCaller, ConversationDependencies, ConversationLimits, ConversationService,
    ProviderSessionErasers, RequestedConversation, SubmissionMode, SubmittedMessage,
    SubmittedUpload,
};
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit, MemoryAttachments,
    MemoryListing, MemoryRepository, MemorySummaries, Provider, ProviderFactory,
    RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
};
use nessa_sdk::{
    domain::common::value_objects::{MediaType, Sha256Digest},
    infrastructure::session_storage::InMemoryStorage,
};
use sha2::{Digest, Sha256};

pub(in crate::conversation::application) struct Fixture {
    #[cfg(unix)]
    _public_parent: tempfile::TempDir,
    directory: tempfile::TempDir,
    pub(in crate::conversation::application) service: ConversationService,
    provider: Arc<ProviderFactory>,
    attachments: Arc<MemoryAttachments>,
    audit: Arc<RecordingFileLinkAudit>,
    pub(in crate::conversation::application) id: ConversationId,
}
pub(in crate::conversation::application) fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "phone".into(),
        action_id: action.into(),
    }
}
impl Fixture {
    pub(in crate::conversation::application) async fn new() -> Self {
        #[cfg(unix)]
        let public_parent = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let directory = {
            use std::os::unix::fs::PermissionsExt;
            // A chosen workspace is the trust root, not its writable locator.
            std::fs::set_permissions(public_parent.path(), std::fs::Permissions::from_mode(0o777))
                .unwrap();
            let directory = tempfile::tempdir_in(public_parent.path()).unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            directory
        };
        #[cfg(not(unix))]
        let directory = tempfile::tempdir().unwrap();
        let provider = Arc::new(ProviderFactory::default());
        let attachments = Arc::new(MemoryAttachments::default());
        let repository = Arc::new(MemoryRepository::default());
        let summaries = Arc::new(MemorySummaries::default());
        let audit = Arc::new(RecordingFileLinkAudit::default());
        let service = ConversationService::new(
            ConversationDependencies {
                input_files: Some(Arc::new(
                    crate::conversation::application::InputFileDelivery {
                        local: Arc::new(crate::conversation::infrastructure::LocalInputFiles),
                        timer: Arc::new(crate::timer::TokioTimer::new()),
                    },
                )),
                agents: only(Arc::new(Provider::new(provider.clone()))),
                storage: Arc::new(InMemoryStorage::new()),
                metadata: repository.clone(),
                creation_audit: Arc::new(AcceptingCreationAudit),
                mode_audit: Arc::new(AcceptingModeAudit),
                file_link_audit: audit.clone(),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                attachments: Some(attachments.clone()),
                summaries: summaries.clone(),
                listing: Arc::new(MemoryListing {
                    repository,
                    summaries,
                }),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(
                    nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
                ),
                environment: crate::conversation::infrastructure::in_process_environment().into(),
                clock: Arc::new(TestClock),
                access_policy: crate::conversation_test_support::access_policy(),
            },
            ConversationLimits::default(),
            Some(
                std::fs::canonicalize(directory.path())
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .into(),
            ),
        )
        .unwrap();
        let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
        service
            .create(
                id.clone(),
                caller("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        Self {
            #[cfg(unix)]
            _public_parent: public_parent,
            directory,
            service,
            provider,
            attachments,
            audit,
            id,
        }
    }
    pub(in crate::conversation::application) fn keep(
        &self,
        bytes: &[u8],
        media: &str,
        name: &str,
    ) -> UploadedFileReference {
        let file = UploadedFileReference::new(
            Sha256Digest::from_bytes(Sha256::digest(bytes).into()),
            MediaType::parse(media).unwrap(),
            bytes.len() as u64,
            name.into(),
        )
        .unwrap();
        self.attachments.uploads.lock().unwrap().push((
            caller("keep").organization_id,
            self.id.clone(),
            file.clone(),
        ));
        self.attachments.files.lock().unwrap().push((
            self.id.clone(),
            file.digest().to_hex(),
            bytes.to_vec(),
        ));
        file
    }
    async fn send(
        &self,
        execution: &str,
        file: &UploadedFileReference,
    ) -> Result<(), super::super::ConversationError> {
        self.service
            .submit(
                self.id.clone(),
                caller(execution),
                execution.into(),
                SubmittedMessage {
                    text: String::new(),
                    images: vec![],
                    files: vec![],
                    uploads: vec![SubmittedUpload {
                        digest: file.digest().to_string(),
                        media_type: file.media_type().as_str().into(),
                        size: file.size(),
                        name: file.name().into(),
                    }],
                },
                SubmissionMode::Queue,
            )
            .await
            .map(|_| ())
    }
    pub(in crate::conversation::application) async fn ended(
        &self,
        execution: &str,
    ) -> nessa_protocol::conversation::view::ConversationView {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let view = self.service.read(self.id.clone(), caller("read")).await.unwrap();
                if view.messages.iter().any(|message| message.execution_id == execution && !matches!(message.status, nessa_protocol::conversation::view::ConversationMessageStatus::Running | nessa_protocol::conversation::view::ConversationMessageStatus::Queued)) { return view; }
                tokio::task::yield_now().await;
            }
        }).await.expect("uploaded turn settles")
    }
}

#[cfg(unix)]
#[tokio::test]
async fn files_only_are_placed_before_dispatch_and_views_keep_original_references() {
    let fixture = Fixture::new().await;
    for (index, (media, name, bytes)) in [
        ("application/pdf", "report.pdf", b"%PDF fixture".as_slice()),
        ("application/zip", "archive.zip", b"PK fixture".as_slice()),
        ("text/plain", "notes.txt", b"hello file".as_slice()),
    ]
    .into_iter()
    .enumerate()
    {
        let execution = format!("file-{index}");
        let file = fixture.keep(bytes, media, name);
        fixture.send(&execution, &file).await.unwrap();
        let view = fixture.ended(&execution).await;
        if index == 0 {
            let listed = fixture.service.list(caller("list"), false).await.unwrap();
            assert_eq!(listed.conversations[0].title.as_deref(), Some(name));
        }
        let turn = view
            .messages
            .iter()
            .find(|message| message.execution_id == execution)
            .unwrap();
        assert_eq!(
            turn.status,
            nessa_protocol::conversation::view::ConversationMessageStatus::Completed
        );
        assert_eq!(turn.uploads.len(), 1);
        assert_eq!(turn.uploads[0].name, name);
        assert_eq!(turn.uploads[0].digest, file.digest().to_string());
        assert!(turn.files.is_empty());
        let path = std::fs::canonicalize(fixture.directory.path())
            .unwrap()
            .join(file.destination_name());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let messages = fixture.provider.messages.lock().unwrap();
        assert_eq!(messages[index].files()[0].path(), path.to_str().unwrap());
        assert!(messages[index].uploads().is_empty());
        let records = fixture.audit.input_records.lock().unwrap();
        let records: Vec<_> = records
            .iter()
            .filter(|record| record.execution == execution)
            .collect();
        assert_eq!(
            records
                .iter()
                .map(|record| record.stage)
                .collect::<Vec<_>>(),
            [
                InputFileStage::Submitted,
                InputFileStage::Asked,
                InputFileStage::Confirmed
            ]
        );
        assert!(records[0].initiator.is_some());
        assert!(records[1].initiator.is_none());
        assert_eq!(records[1].lease, records[2].lease);
        assert_eq!(records[2].file, file);
        assert!(records[2].delivered);
    }
}

#[tokio::test]
async fn a_foreign_or_mismatched_reference_is_refused_before_placement() {
    let fixture = Fixture::new().await;
    let original = fixture.keep(b"hello", "text/plain", "hello.txt");
    for file in [
        UploadedFileReference::new(
            original.digest(),
            MediaType::parse("application/pdf").unwrap(),
            original.size(),
            original.name().into(),
        )
        .unwrap(),
        UploadedFileReference::new(
            original.digest(),
            original.media_type().clone(),
            original.size() + 1,
            original.name().into(),
        )
        .unwrap(),
        UploadedFileReference::new(
            Sha256Digest::from_bytes([3; 32]),
            original.media_type().clone(),
            original.size(),
            original.name().into(),
        )
        .unwrap(),
    ] {
        assert!(matches!(
            fixture.send("bad", &file).await,
            Err(super::super::ConversationError::AttachmentNotFound)
        ));
    }
    assert!(fixture.provider.messages.lock().unwrap().is_empty());
    assert!(fixture.audit.input_records.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(fixture.directory.path()).unwrap().count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_saved_retry_keeps_original_input_and_reads_no_released_upload() {
    let fixture = Fixture::new().await;
    let file = fixture.keep(b"hello", "text/plain", "hello.txt");
    fixture.send("one", &file).await.unwrap();
    fixture.ended("one").await;
    fixture.attachments.uploads.lock().unwrap().clear();
    fixture.attachments.files.lock().unwrap().clear();
    let reads = fixture.attachments.ranges.lock().unwrap().len();
    fixture.send("one", &file).await.unwrap();
    assert_eq!(fixture.provider.messages.lock().unwrap().len(), 1);
    assert_eq!(fixture.attachments.ranges.lock().unwrap().len(), reads);
    let changed = UploadedFileReference::new(
        file.digest(),
        file.media_type().clone(),
        file.size(),
        "other.txt".into(),
    )
    .unwrap();
    assert!(fixture.send("one", &changed).await.is_err());
    assert_eq!(fixture.provider.messages.lock().unwrap().len(), 1);
    assert_eq!(fixture.audit.input_records.lock().unwrap().len(), 3);
}

#[cfg(unix)]
#[tokio::test]
async fn corrupt_stored_bytes_fail_without_a_provider_prompt_or_published_file() {
    let fixture = Fixture::new().await;
    let file = fixture.keep(b"hello", "text/plain", "hello.txt");
    fixture.attachments.files.lock().unwrap()[0].2 = b"wrong".to_vec();
    fixture.send("bad-bytes", &file).await.unwrap();
    fixture.ended("bad-bytes").await;
    assert!(fixture.provider.messages.lock().unwrap().is_empty());
    assert!(!fixture
        .directory
        .path()
        .join(file.destination_name())
        .exists());
    let records = fixture.audit.input_records.lock().unwrap();
    assert_eq!(
        records.last().unwrap().failure,
        Some(UserFileError::Mismatch)
    );
    assert_eq!(records.last().unwrap().stage, InputFileStage::Failed);
}

#[tokio::test]
async fn failed_submission_evidence_prevents_placement_and_dispatch() {
    let fixture = Fixture::new().await;
    let file = fixture.keep(b"hello", "text/plain", "hello.txt");
    fixture.audit.refuses.store(true, Ordering::SeqCst);
    assert!(fixture.send("no-audit", &file).await.is_err());
    assert!(fixture.provider.messages.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(fixture.directory.path()).unwrap().count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_fifo_destination_is_refused_without_blocking_placement() {
    let fixture = Fixture::new().await;
    let file = fixture.keep(b"file bytes", "text/plain", "fifo.txt");
    let name = fixture.directory.path().join(file.destination_name());
    let path = std::ffi::CString::new(name.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    fixture.send("fifo-input", &file).await.unwrap();
    let view = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let view = fixture
                .service
                .read(fixture.id.clone(), caller("read"))
                .await
                .unwrap();
            if view.messages.iter().any(|message| {
                message.execution_id == "fifo-input"
                    && message.status
                        == nessa_protocol::conversation::view::ConversationMessageStatus::Failed
            }) {
                break view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Unblock a regressed blocking open before reporting a deadline failure.
    if view.is_err() {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&name);
    }
    assert!(view.is_ok(), "occupied FIFO blocked delivery");
    assert!(fixture.provider.messages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_hanging_audit_has_a_bounded_wait() {
    struct Hanging;
    impl ConversationFileLinkAudit for Hanging {
        fn record(
            &self,
            _: super::super::ConversationFileLinkAuditRecord,
        ) -> super::super::ConversationFuture<'_, ()> {
            Box::pin(std::future::pending())
        }
        fn record_input(&self, _: InputFileAudit) -> super::super::ConversationFuture<'_, ()> {
            Box::pin(std::future::pending())
        }
    }
    let fixture = Fixture::new().await;
    let file = fixture.keep(b"bounded evidence", "text/plain", "audit.txt");
    let timer = Arc::new(crate::timer::manual::ManualTimer::default());
    let clock = timer.clone();
    let record = InputFileAudit {
        conversation: fixture.id,
        organization: caller("audit").organization_id,
        execution: "audit-turn".into(),
        lease: None,
        attempt: "attempt".into(),
        file,
        stage: InputFileStage::Asked,
        destination: None,
        delivered: false,
        failure: None,
        initiator: None,
        observed_at_ms: 0,
    };
    let write = tokio::spawn(async move { record_input(clock.as_ref(), &Hanging, record).await });
    let wait = timer.started(|_, _| true).await;
    assert_eq!(wait.limit(), std::time::Duration::from_secs(10));
    timer.advance_to(wait.deadline);
    assert!(matches!(
        write.await.unwrap(),
        Err(super::super::ConversationError::Audit)
    ));
}

#[cfg(not(unix))]
#[tokio::test]
async fn unsupported_local_placement_is_refused_before_any_file_effect() {
    let fixture = Fixture::new().await;
    let file = fixture.keep(b"hello", "text/plain", "hello.txt");
    fixture.send("unsupported-host", &file).await.unwrap();
    let view = fixture.ended("unsupported-host").await;
    assert_eq!(view.messages[0].uploads[0].name, "hello.txt");
    assert!(fixture.provider.messages.lock().unwrap().is_empty());
    assert!(fixture.attachments.ranges.lock().unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(fixture.directory.path()).unwrap().count(),
        0
    );
    let records = fixture.audit.input_records.lock().unwrap();
    let ended = records.last().unwrap();
    assert_eq!(ended.stage, InputFileStage::Failed);
    assert_eq!(ended.failure, Some(UserFileError::NotOffered));
    assert!(!ended.delivered);
}
