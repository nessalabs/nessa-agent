//! One accepted restore for the conversation stress check.
//!
//! Each count is two deferred conversations in one oversized session file.
//! The check opens the loaded one while that file is still there, then moves
//! the file aside and sends in the other. The draft of that send stays.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nessa_local_database::rusqlite::{params, Connection};
use nessa_local_database::Schema;
use nessa_local_storage::{create_directory, open, OpenMode};
use nessa_sdk::application::agent_execution::{
    executions::ExecutionRequest,
    permissions::ActionContext,
    providers::ProviderIdentity,
    sessions::{
        InvocationRecord, SessionChange, SessionSaveUnit, SessionStorage, SubmissionAcknowledgement,
    },
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, Initiation, SubmissionMode,
};
use nessa_sdk::domain::agent_execution::prompts::{PromptText, UserMessage};
use nessa_sdk::domain::agent_execution::sessions::{ProviderContext, SessionId};
use nessa_sdk::infrastructure::session_storage::RecordStorage;

use crate::backup::application::{
    AdmissionFailure, AdmissionLease, ExportClock, MutationAdmission,
};
use crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT;
use crate::backup::infrastructure::{capture_namespace, DirectoryPublisher, FileDeletionLog};

const HISTORY: &str = "restored-history";
/// Shown in the transcript. The list preview is [`HISTORY`], so the window
/// check can tell the loaded turn from the row.
const TURN: &str = "restored turn text";

pub struct StressConversation {
    pub id: String,
    pub title: String,
}

pub struct StressRestore {
    pub source: PathBuf,
    pub loaded: Vec<StressConversation>,
    pub missing: Vec<StressConversation>,
}

struct OpenAdmission;

#[async_trait]
impl MutationAdmission for OpenAdmission {
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure> {
        Ok(Box::new(OpenHold))
    }
}

struct OpenHold;

impl AdmissionLease for OpenHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

struct Clock;

impl ExportClock for Clock {
    fn unix_ms(&self) -> u64 {
        8
    }
}

/// Build the source cut, publish its bundle, and return the mac key path.
///
/// `destination` keeps the source file. The caller stages and accepts it.
pub fn build_stress_restore(destination: &Path, count: u32) -> Result<StressRestore, String> {
    if count == 0 {
        return Ok(StressRestore {
            source: destination.join("source").join("records.sqlite3"),
            loaded: Vec::new(),
            missing: Vec::new(),
        });
    }
    let source_dir = destination.join("source");
    create_directory(&source_dir).map_err(|error| error.to_string())?;
    let source = source_dir.join("records.sqlite3");
    let metadata_dir = destination.join("captured-namespace").join("conversations");
    create_directory(&metadata_dir).map_err(|error| error.to_string())?;
    create_directory(&metadata_dir.join("sessions")).map_err(|error| error.to_string())?;
    let grants_dir = destination.join("captured-namespace").join("access-grants");
    create_directory(&grants_dir).map_err(|error| error.to_string())?;
    let loaded = (0..count).map(|slot| named(1, slot)).collect::<Vec<_>>();
    let missing = (0..count).map(|slot| named(2, slot)).collect::<Vec<_>>();
    write_metadata(
        &metadata_dir.join("metadata.sqlite3"),
        &grants_dir.join("access-grants.sqlite3"),
        &loaded,
        &missing,
    )?;
    write_sessions(
        &metadata_dir.join("sessions").join("records.sqlite3"),
        &loaded,
        &missing,
    )?;
    // The exporter reads this path. Keep a second copy the check can move.
    fs::copy(
        metadata_dir.join("sessions").join("records.sqlite3"),
        &source,
    )
    .map_err(|error| error.to_string())?;
    let bundle = destination.join("bundle");
    let inventory =
        FileDeletionLog::open(destination.join("inventory")).map_err(|error| error.to_string())?;
    let publisher = DirectoryPublisher::new(&bundle);
    let namespace = destination.join("captured-namespace");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime
        .block_on(capture_namespace(
            &namespace,
            &OpenAdmission,
            &inventory,
            &publisher,
            &Clock,
            &inventory.key(),
        ))
        .map_err(|error| format!("{error:?}"))?;
    let key_file = destination.join("mac.key");
    let mut key = open(&key_file, OpenMode::CreateNew).map_err(|error| error.to_string())?;
    key.write_all(&inventory.key().0)
        .map_err(|error| error.to_string())?;
    Ok(StressRestore {
        source,
        loaded,
        missing,
    })
}

fn named(kind: u32, slot: u32) -> StressConversation {
    let id = format!("00000000-0000-4000-8000-{kind:04x}{slot:08x}");
    let title = if kind == 1 {
        format!("Restored history {slot}")
    } else {
        format!("Missing source {slot}")
    };
    StressConversation { id, title }
}

fn write_metadata(
    path: &Path,
    grants: &Path,
    loaded: &[StressConversation],
    missing: &[StressConversation],
) -> Result<(), String> {
    let schema = Schema::new(include_str!("../conversation/infrastructure/schema.sql"))
        .map_err(|error| error.to_string())?;
    let connection =
        nessa_local_database::open(path, &schema).map_err(|error| error.to_string())?;
    let grants_schema = Schema::new(include_str!(
        "../conversation/infrastructure/store/grants.sql"
    ))
    .map_err(|error| error.to_string())?;
    drop(nessa_local_database::open(grants, &grants_schema).map_err(|error| error.to_string())?);
    connection
        .execute(
            "INSERT INTO catalogue_owners(organization, owner, head) VALUES ('restored', 'restored', 1)",
            [],
        )
        .map_err(|error| error.to_string())?;
    for conversation in loaded.iter().chain(missing) {
        connection
            .execute(
                "INSERT INTO conversations(
                    id, organization, owner, creator_surface, creation_action,
                    creation_requested_at_ms, agent, model, approval_mode,
                    creation_revision, change_revision
                 ) VALUES (?1, 'restored', 'restored', 'stress', 'restore', 1, 'claude', 'claude-sonnet-5', 'ask', 1, 1)",
                params![conversation.id],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO summaries(conversation_id, title, preview, updated_at_ms, archived)
                 VALUES (?1, ?2, ?3, 1, 0)",
                params![conversation.id, conversation.title, HISTORY],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_sessions(
    path: &Path,
    loaded: &[StressConversation],
    missing: &[StressConversation],
) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| "session directory is missing".to_owned())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let storage = RecordStorage::new(directory).map_err(|error| error.to_string())?;
        storage
            .initialize()
            .await
            .map_err(|error| error.to_string())?;
        for conversation in loaded {
            save_loaded(&storage, &conversation.id).await?;
        }
        for conversation in missing {
            save_opened(&storage, &conversation.id).await?;
        }
        storage
            .shutdown()
            .await
            .map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })?;
    // A table the session reader never copies. Its pages make the file
    // larger than an export will inline, so both conversations are deferred,
    // and a fetch copies only the small records above.
    let database = Connection::open(path).map_err(|error| error.to_string())?;
    let padding = i64::try_from(SESSION_EXPORT_LIMIT).map_err(|error| error.to_string())? + 4096;
    database
        .execute_batch("CREATE TABLE padding (blob BLOB);")
        .map_err(|error| error.to_string())?;
    database
        .execute(
            "INSERT INTO padding (blob) VALUES (zeroblob(?1))",
            params![padding],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn save_opened(storage: &RecordStorage, id: &str) -> Result<(), String> {
    let session = SessionId::new(id).map_err(|error| error.to_string())?;
    let lease = storage
        .open(session.clone())
        .await
        .map_err(|error| error.to_string())?;
    let binding = lease
        .load()
        .await
        .map_err(|error| error.to_string())?
        .binding()
        .clone();
    let opened = SessionSaveUnit::new(vec![SessionChange::Opened {
        id: session,
        provider: ProviderIdentity::new("claude-acp", "claude-sonnet-5", "restored")
            .map_err(|error| error.to_string())?,
        context: ProviderContext::Absent,
    }])
    .map_err(|error| error.to_string())?;
    lease
        .save_changes(binding, vec![opened])
        .await
        .map_err(|error| error.to_string())?;
    drop(lease);
    Ok(())
}

async fn save_loaded(storage: &RecordStorage, id: &str) -> Result<(), String> {
    save_opened(storage, id).await?;
    let session = SessionId::new(id).map_err(|error| error.to_string())?;
    let lease = storage
        .open(session)
        .await
        .map_err(|error| error.to_string())?;
    let binding = lease
        .load()
        .await
        .map_err(|error| error.to_string())?
        .binding()
        .clone();
    let execution = ExecutionId::new("restored-turn").map_err(|error| error.to_string())?;
    let record = InvocationRecord {
        decline: None,
        cancellation_delivery: None,
        resumed_from: None,
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: execution.clone(),
            user_message: UserMessage::text_only(
                PromptText::new(TURN).map_err(|error| error.to_string())?,
            ),
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("person", "panel", "restored-turn")
            .map_err(|error| error.to_string())?,
        initiation: Initiation::owner(),
        notes: Vec::new(),
        handoff: None,
        acknowledgement: SubmissionAcknowledgement::Acknowledged,
        events: Vec::new(),
        scheduling: Vec::new(),
        cancellation: None,
        provider_report: None,
        local_cancellation: None,
        local_outcome: Some(ExecutionOutcome::Completed),
        result: Some(Ok(ExecutionOutcome::Completed)),
    };
    let mut input = record.clone();
    input.result = None;
    input.local_outcome = None;
    let mut changes = vec![SessionChange::InputAccepted(Box::new(input))];
    changes.push(SessionChange::LocalSettlement {
        execution_id: execution,
        before: None,
        after: Ok(ExecutionOutcome::Completed),
        local_outcome: Some(ExecutionOutcome::Completed),
    });
    let unit = SessionSaveUnit::new(changes).map_err(|error| error.to_string())?;
    lease
        .save_changes(binding, vec![unit])
        .await
        .map_err(|error| error.to_string())?;
    drop(lease);
    Ok(())
}
