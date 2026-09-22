//! Messages that point the agent at files by path, through the real SDK Agent:
//! what the gateway checks, what it deliberately does not, what it records
//! before it admits anything, and what a view echoes back.
use super::{
    ConversationCaller, ConversationDependencies, ConversationError, ConversationFileLinkCause,
    ConversationFileLinkState, ConversationLimits, ConversationMessageStatus, ConversationService,
    SubmissionMode, SubmittedFile, SubmittedMessage,
};
use crate::{
    conversation::{domain::ConversationId, infrastructure::DurableConversationFileLinkAudit},
    conversation_test_support::{
        only, AcceptingCreationAudit, MemoryRepository, Provider, ProviderFactory,
        RecordingFileLinkAudit, TestClock,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::sessions::{
        SessionSnapshot, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
    },
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::InMemoryStorage,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn new_id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}
fn caller(surface: &str, action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: surface.into(),
        action_id: action.into(),
    }
}

/// A conversation whose file-link records this test can read back. Nothing
/// about attachments is wired: a path needs no upload store, which is half of
/// why it exists.
async fn conversation() -> (
    ConversationService,
    Arc<RecordingFileLinkAudit>,
    ConversationId,
) {
    let provider = Arc::new(ProviderFactory::default());
    provider.image_input.store(false, Ordering::SeqCst);
    let audit = Arc::new(RecordingFileLinkAudit::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider))),
            storage: Arc::new(InMemoryStorage::new()),
            metadata: Arc::new(MemoryRepository::default()),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: audit.clone(),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        // The launch workspace. Every path below is outside it on purpose.
        Some("/workspace".into()),
    )
    .unwrap();
    let id = new_id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();
    (service, audit, id)
}

async fn send(
    service: &ConversationService,
    id: &ConversationId,
    execution: &str,
    text: &str,
    paths: &[&str],
) -> Result<(), ConversationError> {
    attempt(service, id, execution, execution, text, paths).await
}

/// `send`, with the attempt named apart from the submission. On the wire those
/// are two identifiers: the submission is the caller's execution identity, and
/// the attempt is the request identity, minted afresh every time the panel
/// dispatches — including when it dispatches the same submission again.
async fn attempt(
    service: &ConversationService,
    id: &ConversationId,
    action: &str,
    execution: &str,
    text: &str,
    paths: &[&str],
) -> Result<(), ConversationError> {
    service
        .submit(
            id.clone(),
            caller("panel", action),
            execution.into(),
            SubmittedMessage {
                text: text.into(),
                images: Vec::new(),
                files: paths
                    .iter()
                    .map(|path| SubmittedFile {
                        path: (*path).to_string(),
                    })
                    .collect(),
            },
            SubmissionMode::Queue,
        )
        .await
        .map(|_| ())
}

#[tokio::test]
async fn a_message_can_point_at_a_file_outside_the_workspace_and_the_view_says_which() {
    let (service, audit, id) = conversation().await;

    // Outside the launch workspace, and with a name that has to survive whole.
    send(
        &service,
        &id,
        "e1",
        "read this",
        &["/Users/ada/report (final).pdf"],
    )
    .await
    .unwrap();

    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    let message = view
        .messages
        .iter()
        .find(|message| message.execution_id == "e1")
        .expect("the turn is in the view");
    assert_eq!(message.user_text, "read this");
    // The path is echoed exactly, and nothing was uploaded for it.
    assert_eq!(
        message
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["/Users/ada/report (final).pdf"]
    );
    assert!(message.attachments.is_empty());

    // A message of a file alone is a message: there is nothing else to say.
    send(&service, &id, "e2", "", &["/etc/hosts"])
        .await
        .unwrap();
    let view = service.read(id, caller("panel", "read")).await.unwrap();
    let message = view
        .messages
        .iter()
        .find(|message| message.execution_id == "e2")
        .expect("the turn is in the view");
    assert_eq!(message.user_text, "");
    assert_eq!(
        message
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["/etc/hosts"]
    );
    // Both are ordinary turns; nothing about a path makes one wait.
    assert!(matches!(
        message.status,
        ConversationMessageStatus::Queued
            | ConversationMessageStatus::Running
            | ConversationMessageStatus::Completed
    ));
    let _ = audit;
}

#[tokio::test]
async fn pointing_the_agent_at_files_is_recorded_before_the_message_is_admitted() {
    let (service, audit, id) = conversation().await;
    send(
        &service,
        &id,
        "e1",
        "look",
        &["/Users/ada/a.pdf", "/Users/ada/b.pdf"],
    )
    .await
    .unwrap();

    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.conversation_id, id);
    assert_eq!(record.organization_id.as_str(), "org");
    assert_eq!(record.execution_id, "e1");
    // Every path, in attachment order — not a count, and not the first one.
    assert_eq!(record.paths, ["/Users/ada/a.pdf", "/Users/ada/b.pdf"]);
    assert_eq!(record.before, ConversationFileLinkState::NotNamed);
    assert_eq!(record.after, ConversationFileLinkState::Named);
    assert_eq!(record.cause, ConversationFileLinkCause::CallerSubmitted);
    // The verified caller, not the client's word for itself.
    assert_eq!(record.initiator_principal_id.as_str(), "person");
    assert_eq!(record.initiator_surface_id, "panel");
    // Nothing here names the attempt. The submission is what correlates this
    // record, and it is already the target; the wire's request identity names
    // one attempt at that submission, and attempts differ while the naming
    // does not.
    assert_eq!(record.observed_at_ms, 1_700_000_000_123);
}

#[tokio::test]
async fn a_message_that_points_at_nothing_records_nothing() {
    let (service, audit, id) = conversation().await;
    send(&service, &id, "e1", "just text", &[]).await.unwrap();
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn evidence_that_cannot_be_committed_refuses_the_message() {
    let (service, audit, id) = conversation().await;
    audit.refuses.store(true, Ordering::SeqCst);

    assert!(matches!(
        send(&service, &id, "e1", "look", &["/Users/ada/a.pdf"]).await,
        Err(ConversationError::Audit)
    ));

    // Refused before admission, so the agent never heard of it.
    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert!(view
        .messages
        .iter()
        .all(|message| message.execution_id != "e1"));
    assert!(view
        .pending
        .iter()
        .all(|pending| pending.execution_id != "e1"));

    // A message with no files is untouched by the same failing sink: nothing
    // about it needs this evidence.
    send(&service, &id, "e2", "just text", &[]).await.unwrap();
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_path_that_could_not_be_carried_faithfully_is_refused_before_anything_is_recorded() {
    let (service, audit, id) = conversation().await;
    for (case, path) in [
        ("relative", "notes/report.pdf"),
        ("home-relative", "~/report.pdf"),
        ("newline", "/Users/ada/a\nb.pdf"),
        ("nul", "/Users/ada/a\u{0}b.pdf"),
        ("a directory", "/Users/ada/"),
        ("the root", "/"),
        ("dot", "/Users/ada/."),
        ("dot dot", "/Users/ada/.."),
        ("a dot component", "/Users/../etc/passwd"),
        ("a doubled separator", "/Users//ada/report.pdf"),
        ("a doubled root", "//Users/ada/report.pdf"),
        ("empty", ""),
    ] {
        assert!(
            matches!(
                send(&service, &id, case, "look", &[path]).await,
                Err(ConversationError::InvalidInput)
            ),
            "{case}: {path:?}"
        );
    }
    // Refused by the domain, so no grant was ever claimed for any of them.
    assert!(audit.records.lock().unwrap().is_empty());

    // And what is not refused: a bracket or a backslash used to be, on the
    // grounds that the agent is handed the path inside a markdown link. The
    // adapter encodes both halves of that link now, so these are ordinary
    // names and the gateway has no opinion about them.
    for (case, path) in [
        ("bracket in the name", "/Users/ada/a]b.pdf"),
        (
            "bracket in a directory",
            "/Users/ada/](file:/etc/passwd) [x/report.pdf",
        ),
        ("a trailing backslash", "/Users/ada/report\\"),
    ] {
        send(&service, &id, case, "look", &[path])
            .await
            .unwrap_or_else(|error| panic!("{case}: {path:?} was refused as {error:?}"));
    }
    audit.records.lock().unwrap().clear();

    // More paths than one message may name.
    let many: Vec<String> = (0..11).map(|n| format!("/Users/ada/{n}.pdf")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert!(matches!(
        send(&service, &id, "many", "look", &many).await,
        Err(ConversationError::InvalidInput)
    ));
    assert!(audit.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_gateway_does_not_ask_whether_the_file_is_there() {
    let (service, audit, id) = conversation().await;

    // Nothing at this path, and nothing under it — the whole point is that the
    // question is answered when the agent opens it, behind a permission, not
    // here where the answer could only go stale.
    let missing = "/Users/ada/does/not/exist/at/all.pdf";
    send(&service, &id, "e1", "read this", &[missing])
        .await
        .unwrap();
    assert_eq!(audit.records.lock().unwrap()[0].paths, [missing]);

    // A directory that does exist is still refused, because it has no file
    // name — that is a rule about the path, not about the filesystem.
    assert!(matches!(
        send(&service, &id, "e2", "read this", &["/tmp/"]).await,
        Err(ConversationError::InvalidInput)
    ));
}

/// The forgery this closes. A submission identity is the caller's, and a repeat
/// of one is the agent's to settle — the same message recovers its original
/// delivery, any other conflicts. Recording a repeat here would write evidence
/// naming paths that repeat never delivered, and when the first submission of
/// that identity named no files there is nothing stored for it to contradict,
/// so it would stand unopposed. A durable record that Nessa pointed an agent at
/// a private key it never sent is worse than no record at all.
#[tokio::test]
async fn a_repeat_of_a_file_less_submission_cannot_plant_a_path_in_the_record() {
    let (service, audit, id) = conversation().await;

    // An ordinary text message, admitted under `e1`, recording nothing.
    send(&service, &id, "e1", "hello", &[]).await.unwrap();
    assert!(audit.records.lock().unwrap().is_empty());

    // The same identity again, now naming a file nobody attached. The agent
    // already has `e1`, so this cannot become a message — and it must not
    // become evidence either.
    assert!(
        send(&service, &id, "e1", "hello", &["/Users/ada/.ssh/id_rsa"])
            .await
            .is_err(),
        "a conflicting repeat must not be admitted"
    );
    {
        let planted = audit.records.lock().unwrap();
        assert!(planted.is_empty(), "a repeat wrote evidence: {planted:?}");
    }

    // The original message is untouched: still one turn, still naming nothing.
    let view = service.read(id, caller("panel", "read")).await.unwrap();
    let message = view
        .messages
        .iter()
        .find(|message| message.execution_id == "e1")
        .expect("the original turn is still there");
    assert_eq!(message.user_text, "hello");
    assert!(message.files.is_empty());
}

#[tokio::test]
async fn a_retried_submission_records_once_because_only_a_new_one_records_at_all() {
    let (service, audit, id) = conversation().await;
    let paths = ["/Users/ada/a.pdf"];

    send(&service, &id, "e1", "look", &paths).await.unwrap();
    // The same submission again. The agent already has it, so this is its retry
    // to recover — and nothing is recorded a second time, because the evidence
    // for this submission was written when it was new.
    send(&service, &id, "e1", "look", &paths).await.unwrap();

    let records = audit.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].execution_id, "e1");
    assert_eq!(records[0].paths, ["/Users/ada/a.pdf"]);
}

/// Storage that will not save the first input an agent tries to admit, and
/// then behaves. That is the shape of a transient write failure at exactly the
/// moment this test needs one: the naming has already been recorded, the
/// message is not admitted, and the panel dispatches the same submission again.
struct RefuseFirstInput {
    refused: Arc<AtomicBool>,
    inner: Arc<InMemoryStorage>,
}
impl SessionStorage for RefuseFirstInput {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        let refused = self.refused.clone();
        let inner = self.inner.clone();
        Box::pin(async move {
            let inner = inner.open(id).await?;
            Ok(Box::new(RefuseFirstInputLease { refused, inner }) as Box<dyn SessionStorageLease>)
        })
    }
}
struct RefuseFirstInputLease {
    refused: Arc<AtomicBool>,
    inner: Box<dyn SessionStorageLease>,
}
impl SessionStorageLease for RefuseFirstInputLease {
    fn load(&self) -> StorageFuture<'_, Option<SessionSnapshot>> {
        self.inner.load()
    }
    fn save(&self, snapshot: SessionSnapshot) -> StorageFuture<'_, ()> {
        // Opening the session saves a snapshot with nothing in it; the first
        // one carrying an input is the admission to refuse.
        if !snapshot.invocations.is_empty() && !self.refused.swap(true, Ordering::SeqCst) {
            return Box::pin(async { Err(StorageError::Io("the disk went away".into())) });
        }
        self.inner.save(snapshot)
    }
}

/// One submission, dispatched twice, because the first dispatch failed after
/// its naming was already recorded. The durable sink sees the same submission
/// twice and has to decide whether the second is the same grant or a
/// contradiction of it.
///
/// The record used to carry the wire's request identity, inside a record keyed
/// by the submission. Those two disagree exactly here: attempts differ while
/// the naming does not, so the second attempt built a differing document, the
/// sink refused it as evidence contradicting the first, and the submission
/// became permanently unsendable — reported to the person as a failure to
/// record something that had been recorded correctly the first time. The
/// retry test above cannot see this, because its helper names the attempt
/// after the submission and so the two facts never cross.
#[tokio::test]
async fn a_second_attempt_at_one_submission_is_not_evidence_against_the_first() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("file-links");
    let provider = Arc::new(ProviderFactory::default());
    provider.image_input.store(false, Ordering::SeqCst);
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider))),
            storage: Arc::new(RefuseFirstInput {
                refused: Arc::new(AtomicBool::new(false)),
                inner: Arc::new(InMemoryStorage::new()),
            }),
            metadata: Arc::new(MemoryRepository::default()),
            creation_audit: Arc::new(AcceptingCreationAudit),
            // The real sink: reconciling a repeat is its rule, and an
            // in-memory recorder would accept anything at all.
            file_link_audit: Arc::new(
                DurableConversationFileLinkAudit::new(directory.clone()).unwrap(),
            ),
            attachments: None,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        Some("/workspace".into()),
    )
    .unwrap();
    let id = new_id();
    service
        .create(id.clone(), caller("panel", "create"), None)
        .await
        .unwrap();

    let paths = ["/Users/ada/a.pdf"];
    // The first dispatch: recorded, then not admitted.
    assert!(
        attempt(&service, &id, "first-attempt", "e1", "look", &paths)
            .await
            .is_err(),
        "the input could not be saved, so the message cannot be admitted"
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

    // The same submission again, under the identity this attempt was given.
    attempt(&service, &id, "second-attempt", "e1", "look", &paths)
        .await
        .expect("a second attempt at one submission is not a contradiction");

    // Still one record, still the first observation of the same naming.
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    let stored: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            std::fs::read_dir(&directory)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(stored["target"]["executionId"], "e1");
    assert_eq!(
        stored["target"]["paths"],
        serde_json::json!(["/Users/ada/a.pdf"])
    );

    // And the message the second attempt admitted is the one in the view.
    let view = service.read(id, caller("panel", "read")).await.unwrap();
    let message = view
        .messages
        .iter()
        .find(|message| message.execution_id == "e1")
        .expect("the submission arrived on its second attempt");
    assert_eq!(
        message
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["/Users/ada/a.pdf"]
    );
}
