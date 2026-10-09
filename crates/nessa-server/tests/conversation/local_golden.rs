//! Golden evidence of one local conversation on the real local stores: the
//! records its stream holds, every audit row written, and the cleanup the
//! agent was asked for and reported. Captured before the `Environment` port
//! existed (#698), so a refactor beneath the conversation service that changes
//! any of it — beyond the lease records that change adds — fails here.
//!
//! The golden file is the evidence. It is rewritten only by running this test
//! with `NESSA_UPDATE_GOLDEN=1`, which a reviewer sees as a diff to it.
use super::{
    ConversationAgent, ConversationAgents, ConversationCaller, ConversationDependencies,
    ConversationLimits, ConversationService, RequestedConversation, SubmissionMode, SubmittedFile,
    SubmittedMessage,
};
use crate::conversation::infrastructure::{
    DurableConversationCreationAudit, DurableConversationDeletionAudit,
    DurableConversationFileLinkAudit, DurableExecutionAudit, LocalConversationStore,
};
use crate::conversation_test_support::{
    claude_erasers, AcceptingModeAudit, Provider, ProviderFactory, TestClock, DELETION_BUDGETS,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::ConversationMessageStatus;
use nessa_protocol::{agents::AgentId, conversation::domain::ConversationId};
use nessa_sdk::application::agent_execution::sessions::{
    CurrentLease, LeaseRecord, SessionStorage,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseCleanup, LeaseEndCause, LeaseRevision};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

const CONVERSATION: &str = "0b7d3c1e-5a4f-4e2b-9c8d-7f6a5b4c3d2e";
const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/conversation/golden/local_conversation.txt"
);

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        action_id: action.into(),
    }
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.insert(path.clone(), std::fs::read(&path).unwrap());
        }
    }
    found
}

/// What one local conversation leaves behind, rendered as text.
struct Evidence {
    /// The committed records of the conversation's stream, as the SDK folds
    /// them, without the lease records `#698` adds.
    records: String,
    /// Every audit row, by path under the audit directory.
    audit: String,
    /// The cleanup the agent was asked for and what it reported.
    cleanup: String,
    /// The lease records `#698` adds, kept out of the golden text and
    /// asserted on their own.
    lease: Option<CurrentLease>,
}

async fn run_local_conversation(root: &Path) -> Evidence {
    let provider = Arc::new(ProviderFactory::default());
    let clock: Arc<dyn Clock> = Arc::new(TestClock);
    let agents = ConversationAgents::new(
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                provider: Arc::new(Provider::new(provider.clone())),
                execution_audit: Arc::new(
                    DurableExecutionAudit::new(root.join("audit"), clock.clone()).unwrap(),
                ),
                reserved_output_tokens: 4096,
                readiness: None,
                sandbox:
                    nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
            },
        )]),
        AgentId::Claude,
    )
    .unwrap();
    nessa_local_storage::create_directory(&root.join("conversations")).unwrap();
    let database = root.join("conversations").join("metadata.sqlite3");
    let metadata = Arc::new(LocalConversationStore::open(&database).unwrap());
    let storage = Arc::new(RecordStorage::new(root.join("sessions")).unwrap());
    let service = ConversationService::new(
        ConversationDependencies {
            agents,
            storage: storage.clone(),
            metadata: metadata.clone(),
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(
                DurableConversationCreationAudit::new(root.join("audit").join("creation")).unwrap(),
            ),
            file_link_audit: Arc::new(
                DurableConversationFileLinkAudit::new(root.join("audit").join("file-links"))
                    .unwrap(),
            ),
            deletion_audit: Arc::new(
                DurableConversationDeletionAudit::new(
                    root.join("audit").join("deletion"),
                    clock.clone(),
                )
                .unwrap(),
            ),
            attachments: None,
            summaries: metadata.clone(),
            listing: metadata.clone(),
            provider_sessions: claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock,
            environment: crate::conversation::infrastructure::in_process_environment(),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = ConversationId::new(CONVERSATION).unwrap();
    service
        .create(
            id.clone(),
            caller("create"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("send-1"),
            "turn-1".into(),
            SubmittedMessage {
                text: "read this".into(),
                images: Vec::new(),
                files: vec![SubmittedFile {
                    path: "/tmp/notes.txt".into(),
                }],
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = service.read(id.clone(), caller("read")).await.unwrap();
            if view.messages.iter().any(|message| {
                message.execution_id == "turn-1"
                    && message.status == ConversationMessageStatus::Completed
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the turn completes");
    service.close(id.clone(), caller("close")).await.unwrap();

    let session = SessionId::new(id.to_string()).unwrap();
    let snapshot = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match storage.open_existing(session.clone()).await {
                Ok(Some(lease)) => break lease.load().await.unwrap().snapshot().cloned(),
                Ok(None) => panic!("the conversation's stream exists"),
                Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await
    .expect("the stream can be read once the agent let it go")
    .expect("the stream holds a snapshot");
    let records = format!(
        "id: {:?}\nprovider: {:#?}\nprovider_context: {:#?}\ninvocations: {:#?}\nqueue_history: {:#?}\n",
        snapshot.id,
        snapshot.provider,
        snapshot.provider_context,
        snapshot.invocations,
        snapshot.queue_history,
    );
    let audit_root = root.join("audit");
    // Execution audit rows are named by a random record identity, so they are
    // put in the order their writer gave them; every other row by its path.
    let mut rows: Vec<(String, String)> = files(&audit_root)
        .into_iter()
        .map(|(path, bytes)| {
            let relative = path.strip_prefix(&audit_root).unwrap();
            let text = String::from_utf8(bytes).unwrap();
            let key = if relative.parent() == Some(Path::new("")) {
                let row: serde_json::Value = serde_json::from_str(&text).unwrap();
                format!("execution/{:08}", row["sequence"].as_u64().unwrap())
            } else {
                relative.display().to_string()
            };
            (key, text)
        })
        .collect();
    rows.sort();
    let mut audit = String::new();
    for (key, text) in rows {
        writeln!(audit, "== {key}\n{text}").unwrap();
    }
    let cleanup = format!(
        "close_calls: {}\nclose_requests: {:#?}\n",
        provider.close_calls.load(Ordering::SeqCst),
        provider.close_requests.lock().unwrap(),
    );
    Evidence {
        records,
        audit,
        cleanup,
        lease: snapshot.lease.clone(),
    }
}

/// The evidence as text, with every random identity other than the
/// conversation's replaced by its order of first appearance, so two runs of
/// the same conversation read the same.
fn render(evidence: &Evidence) -> String {
    let text = format!(
        "### records\n{}\n### audit\n{}\n### cleanup\n{}",
        evidence.records, evidence.audit, evidence.cleanup
    );
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some((index, uuid)) = next_uuid(rest) {
        out.push_str(&rest[..index]);
        if uuid == CONVERSATION {
            out.push_str(uuid);
        } else {
            let ordinal = match seen.iter().position(|known| known == uuid) {
                Some(ordinal) => ordinal,
                None => {
                    seen.push(uuid.to_owned());
                    seen.len() - 1
                }
            };
            write!(out, "<uuid-{ordinal}>").unwrap();
        }
        rest = &rest[index + uuid.len()..];
    }
    out.push_str(rest);
    out
}

/// The first hyphenated lowercase UUID in `text`, and where it starts.
fn next_uuid(text: &str) -> Option<(usize, &str)> {
    let shape = [8, 4, 4, 4, 12];
    (0..text.len()).find_map(|start| {
        let candidate = text.get(start..start + 36)?;
        let mut groups = candidate.split('-');
        let fits = shape.iter().all(|length| {
            groups.next().is_some_and(|group| {
                group.len() == *length
                    && group
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        }) && groups.next().is_none();
        fits.then_some((start, candidate))
    })
}

#[tokio::test]
async fn a_local_conversation_leaves_the_golden_records_audit_and_cleanup_evidence() {
    let root = tempfile::tempdir().unwrap();
    let evidence = render(&run_local_conversation(root.path()).await);
    if std::env::var_os("NESSA_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(Path::new(GOLDEN).parent().unwrap()).unwrap();
        std::fs::write(GOLDEN, &evidence).unwrap();
    }
    let golden = std::fs::read_to_string(GOLDEN).expect("the golden evidence is checked in");
    assert_eq!(evidence, golden);
}

/// Apart from the golden evidence, the run left exactly one lease: issued
/// for the run, ended by the person's close, with the close's confirmation.
#[tokio::test]
async fn a_local_conversation_leaves_one_lease_for_its_run_ended_by_its_close() {
    let root = tempfile::tempdir().unwrap();
    let lease = run_local_conversation(root.path())
        .await
        .lease
        .expect("the run was leased");
    let held = lease.held().expect("the lease was issued");
    assert_eq!(held.revision(), LeaseRevision::FIRST);
    let [LeaseRecord::Issued { .. }, LeaseRecord::Ending {
        cause: LeaseEndCause::Closed,
        actor: Some(actor),
        ..
    }, LeaseRecord::Ended {
        cleanup: LeaseCleanup::Confirmed { forced: false },
        ..
    }] = lease.records()
    else {
        panic!("one issued, closed and ended lease: {:?}", lease.records());
    };
    assert_eq!(actor.request_id(), "close");
}
