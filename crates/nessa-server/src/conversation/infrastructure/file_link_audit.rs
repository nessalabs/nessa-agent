//! Durable evidence that a message pointed an agent at files on this machine,
//! committed before the submission is admitted.
//!
//! An uploaded image already has a record of who put it here, written by the
//! attachments context when the bytes arrived. A path is uploaded nowhere, so
//! without this there would be no record at all that Nessa handed an agent a
//! path — possibly one outside the single directory the agent was launched in.
//! That is the reach this file exists to keep evidence of.
//!
//! ```text
//!   submit ──record──▶ this adapter ──▶ <root>/file-links/<name>.json
//!      │                                          │
//!      └── refuses the submission on failure      └── one file per submission,
//!                                                     private, named from a
//!                                                     digest of the identity
//! ```
//!
//! Arrows are calls and writes. One record per conversation and submission, so
//! a retried submission reconciles the same evidence rather than claiming a
//! second grant; contradictory evidence for the same submission fails closed,
//! exactly as conversation creation does.
//!
//! The name is derived, never copied from input: a submission identity is 256
//! bytes of whatever a caller sent, and a path is what a person chose, so
//! neither may reach the filesystem as a name.
use crate::conversation::application::{
    ConversationError, ConversationFileLinkAudit, ConversationFileLinkAuditRecord,
    ConversationFileLinkCause, ConversationFileLinkState, ConversationFuture,
};
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// Most bytes one stored record may be before it is treated as damaged. Ten
/// paths of 4 KiB each, their JSON escaping, and the envelope around them fit
/// inside this with room to spare.
const MAX_RECORD_BYTES: usize = 131_072;

pub struct DurableConversationFileLinkAudit {
    directory: PathBuf,
}

impl DurableConversationFileLinkAudit {
    pub fn new(directory: PathBuf) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory })
    }
}

impl ConversationFileLinkAudit for DurableConversationFileLinkAudit {
    fn record(&self, record: ConversationFileLinkAuditRecord) -> ConversationFuture<'_, ()> {
        // The submission identity is the caller's text, so it is hashed rather
        // than spelled: the record still names one submission, and nothing a
        // caller writes becomes part of a path.
        let id = format!(
            "conversation-files-{}-{}",
            record.conversation_id,
            digest_of(record.execution_id.as_bytes())
        );
        let value = json!({
            "recordId": id,
            "kind": "conversation_files_named",
            "target": {
                "conversationId": record.conversation_id.to_string(),
                "organizationId": record.organization_id.as_str(),
                "executionId": record.execution_id,
                "paths": record.paths,
            },
            "transition": {
                "before": state(record.before),
                "after": state(record.after),
            },
            "cause": cause(record.cause),
            "initiator": {
                "principalId": record.initiator_principal_id.as_str(),
                "surfaceId": record.initiator_surface_id,
            },
            "observedAtMs": record.observed_at_ms,
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let destination = directory.join(format!("{id}.json"));
                // Reading first is the common path and gives the better
                // failure — "this contradicts what is stored" rather than "the
                // name is taken". It is not what makes this safe: two
                // submissions of the same identity arriving together can both
                // read nothing, and it is `publish` below, which refuses a
                // destination that exists, that settles which one wrote.
                if let Stored::Agrees = stored_evidence(&destination, &value, &id)? {
                    return sync_directory(&directory).map_err(audit("sync a directory"));
                }
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(audit("open a record"))?;
                serde_json::to_writer(file.as_file_mut(), &value).map_err(|error| {
                    tracing::error!(%error, "could not write file-naming evidence");
                    ConversationError::Audit
                })?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(audit("sync a record"))?;
                // Create-only. A second writer of the same identity that got
                // past the read above loses here rather than overwriting, and
                // a lost race is not a failure: the record it would have
                // written is the one already there, so it is read back and
                // held to the same agreement.
                if let Err(error) = file.publish(&destination) {
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        tracing::error!(record = %id, %error, "could not publish a record");
                        return Err(ConversationError::Audit);
                    }
                    // Whatever beat this writer to the name has to say the
                    // same thing. It having since been removed is not proof of
                    // anything, so it is not treated as agreement.
                    return match stored_evidence(&destination, &value, &id)? {
                        Stored::Agrees => {
                            sync_directory(&directory).map_err(audit("sync a directory"))
                        }
                        Stored::Absent => {
                            tracing::error!(
                                record = %id,
                                "a record published first is already gone"
                            );
                            Err(ConversationError::Audit)
                        }
                    };
                }
                sync_directory(&directory).map_err(audit("sync a directory"))
            })
            .await
            .map_err(|_| ConversationError::Audit)?
        })
    }
}

/// Why an audit write failed, said once where it happened. Every one of these
/// answers the caller with the same `Audit`, which is deliberate — a refused
/// submission should not depend on which syscall failed — but a full disk and
/// evidence that contradicts a submission are very different things to be
/// looking at afterwards, so they are not the same line in a log.
fn audit(doing: &'static str) -> impl Fn(std::io::Error) -> ConversationError {
    move |error| {
        tracing::error!(%error, "could not {doing} for file-naming evidence");
        ConversationError::Audit
    }
}

/// The one field two records of one naming may legitimately disagree about:
/// when each writer happened to see it. Everything else is the naming itself,
/// and a difference in any of it is a contradiction.
///
/// Named once because it is the whole of what "the same evidence" permits to
/// vary, and it used to be spelled into two copies of the comparison below —
/// so adding a second such field meant remembering both, and forgetting one
/// would have made a retry look like a forgery.
const MAY_DIFFER_BETWEEN_WRITERS: &str = "observedAtMs";

/// What is already stored for this submission.
enum Stored {
    /// Nothing is there. The caller writes.
    Absent,
    /// A record is there and says the same thing this one would.
    Agrees,
}

/// Read what is stored at `destination` and hold it to the same agreement this
/// writer would have been held to.
///
/// One reader, used both before publishing and after losing the race to
/// publish, because those two are the same question asked at two moments. They
/// were two functions, each with its own bounded read, its own parse and its
/// own idea of which field may differ; that is one place too many to remember
/// what makes two records the same naming.
///
/// # Errors
///
/// [`ConversationError::Audit`] when the stored record cannot be read, is
/// larger than any this writes, is not the JSON this writes, or disagrees with
/// `value` about anything but [`MAY_DIFFER_BETWEEN_WRITERS`].
fn stored_evidence(
    destination: &Path,
    value: &serde_json::Value,
    id: &str,
) -> Result<Stored, ConversationError> {
    let mut file = match open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Stored::Absent),
        Err(error) => {
            tracing::error!(record = %id, %error, "could not read stored evidence");
            return Err(ConversationError::Audit);
        }
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(audit("read a record"))?;
    if bytes.len() > MAX_RECORD_BYTES {
        tracing::error!(record = %id, "a stored record is larger than any this writes");
        return Err(ConversationError::Audit);
    }
    let stored: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::Audit)?;
    if comparable(stored) != comparable(value.clone()) {
        tracing::error!(
            record = %id,
            "stored file-naming evidence contradicts this submission"
        );
        return Err(ConversationError::Audit);
    }
    Ok(Stored::Agrees)
}

/// `value` with the field two writers may honestly differ on removed.
fn comparable(mut value: serde_json::Value) -> serde_json::Value {
    let _ = value
        .as_object_mut()
        .and_then(|object| object.remove(MAY_DIFFER_BETWEEN_WRITERS));
    value
}

fn digest_of(bytes: &[u8]) -> String {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into()).to_hex()
}

fn state(value: ConversationFileLinkState) -> &'static str {
    match value {
        ConversationFileLinkState::NotNamed => "not_named",
        ConversationFileLinkState::Named => "named",
    }
}

fn cause(value: ConversationFileLinkCause) -> &'static str {
    match value {
        ConversationFileLinkCause::CallerSubmitted => "caller_submitted",
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/file_link_audit.rs"]
mod tests;
