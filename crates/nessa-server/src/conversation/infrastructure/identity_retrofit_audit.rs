//! Durable evidence of the one-shot identity retrofit, one immutable JSON file
//! per fact, in the shape of the approval-mode audit. Temporary, with the
//! retrofit (`conversation::application::identity_retrofit`).
//!
//! A conversation's records are keyed by a digest of their content, so a
//! retried run that records the same intent again is acknowledged rather than
//! duplicated, and a record contradicting a stored one under the same key is
//! refused. A run summary is keyed by its content and the time this adapter
//! observed it, so each run keeps its own. Each file is written to a private
//! temporary file, synced, published without replacing, and its directory
//! synced before the record is acknowledged.
use crate::conversation::application::identity_retrofit::{
    IdentityRetrofitAudit, Leftover, PermanentLeftover, RetrofitAuditRecord, RetrofitCause,
    RetrofitInitiator, RetrofitMove, RetrofitSummary, TransientLeftover,
};
use crate::conversation::application::{ConversationError, ConversationFuture};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// The largest record read back to compare: a summary names every
/// conversation it left, so it is allowed far more than one conversation's.
const MAX_RECORD_BYTES: u64 = 16 * 1024 * 1024;

pub struct DurableIdentityRetrofitAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableIdentityRetrofitAudit {
    /// # Errors
    /// [`ConversationError::Audit`] when the directory cannot be made.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}

fn identity(value: &ProviderIdentity) -> Value {
    json!({
        "name": value.name(),
        "modelId": value.model_id(),
        "context": value.context(),
    })
}

fn leftover(reason: Leftover) -> Value {
    let (kind, name) = match reason {
        Leftover::Permanent(reason) => (
            "permanent",
            match reason {
                PermanentLeftover::UnsupportedAgent => "unsupported_agent",
                PermanentLeftover::AgentNotConfigured => "agent_not_configured",
                PermanentLeftover::ModelUnavailable => "model_unavailable",
                PermanentLeftover::ApprovalModeUnavailable => "approval_mode_unavailable",
                PermanentLeftover::Corrupt => "corrupt",
            },
        ),
        Leftover::Transient(reason) => (
            "transient",
            match reason {
                TransientLeftover::Busy => "busy",
                TransientLeftover::Storage => "storage",
                TransientLeftover::Metadata => "metadata",
                TransientLeftover::Unavailable => "unavailable",
                TransientLeftover::Audit => "audit",
            },
        ),
    };
    json!({"kind": kind, "reason": name})
}

fn transition(phase: &str, moved: &RetrofitMove, reason: Option<Leftover>) -> Value {
    json!({
        "kind": "conversation_restoration_identity",
        "target": {"conversationId": moved.conversation_id.to_string()},
        "transition": {
            "before": identity(&moved.before),
            "after": identity(&moved.after),
            "phase": phase,
            "left": reason.map(leftover),
        },
        "cause": cause(moved.cause),
        "initiator": initiator(moved.initiator),
    })
}

fn cause(value: RetrofitCause) -> &'static str {
    match value {
        RetrofitCause::McpServersLeftRestorationIdentity => "mcp_servers_left_restoration_identity",
    }
}

fn initiator(value: RetrofitInitiator) -> Value {
    match value {
        RetrofitInitiator::SystemGatewayStart => {
            json!({"kind": "system", "event": "gateway_start"})
        }
    }
}

fn summary(value: &RetrofitSummary) -> Value {
    json!({
        "kind": "conversation_restoration_identity_run",
        "counts": {
            "rewritten": value.rewritten,
            "alreadyCurrent": value.already_current,
            "foreign": value.foreign,
            "noHistory": value.no_history,
            "tombstoned": value.tombstoned,
            "unreadableRecords": value.unreadable_records,
            "left": value.left.len(),
        },
        "listingFailed": value.listing_failed,
        "left": value.left.iter().map(|(id, reason)| json!({
            "conversationId": id.to_string(),
            "left": leftover(*reason),
        })).collect::<Vec<_>>(),
        "cause": cause(RetrofitCause::OF_RUN),
        "initiator": initiator(RetrofitInitiator::OF_RUN),
    })
}

impl IdentityRetrofitAudit for DurableIdentityRetrofitAudit {
    fn record(&self, record: RetrofitAuditRecord) -> ConversationFuture<'_, ()> {
        let observed = self.clock.unix_milliseconds();
        let (mut value, keyed_by_time) = match &record {
            RetrofitAuditRecord::Rewriting(moved) => (transition("rewriting", moved, None), false),
            RetrofitAuditRecord::Rewritten(moved) => (transition("rewritten", moved, None), false),
            RetrofitAuditRecord::Left { attempted, reason } => {
                (transition("left", attempted, Some(*reason)), false)
            }
            RetrofitAuditRecord::Summary(run) => (summary(run), true),
        };
        let mut digest = Sha256::new();
        digest.update(value.to_string().as_bytes());
        if keyed_by_time {
            digest.update(observed.to_be_bytes());
        }
        let id = format!("fingerprint-retrofit-{:x}", digest.finalize());
        let object = value.as_object_mut().expect("records are objects");
        object.insert("recordId".into(), json!(id));
        // When this adapter observed the fact for durable delivery, not the
        // unknown time of the append it describes.
        object.insert("observedAtMs".into(), json!(observed));
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let destination = directory.join(format!("{id}.json"));
                if agrees(&destination, &value)? {
                    return sync_directory(&directory).map_err(|_| ConversationError::Audit);
                }
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|_| ConversationError::Audit)?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|_| ConversationError::Audit)?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| ConversationError::Audit)?;
                if let Err(error) = file.publish(&destination) {
                    if error.kind() != ErrorKind::AlreadyExists || !agrees(&destination, &value)? {
                        return Err(ConversationError::Audit);
                    }
                }
                sync_directory(&directory).map_err(|_| ConversationError::Audit)
            })
            .await
            .map_err(|_| ConversationError::Audit)?
        })
    }
}

/// Whether the record already stored under this key says the same, apart from
/// when it was observed. Absent is `false`; a different record is refused.
fn agrees(destination: &Path, expected: &Value) -> Result<bool, ConversationError> {
    let mut file = match open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ConversationError::Audit),
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConversationError::Audit)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(ConversationError::Audit);
    }
    let mut stored: Value = serde_json::from_slice(&bytes).map_err(|_| ConversationError::Audit)?;
    let mut expected = expected.clone();
    for value in [&mut stored, &mut expected] {
        if let Some(object) = value.as_object_mut() {
            object.remove("observedAtMs");
        }
    }
    if stored != expected {
        return Err(ConversationError::Audit);
    }
    Ok(true)
}
