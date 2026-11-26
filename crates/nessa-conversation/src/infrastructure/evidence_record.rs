//! One durable evidence record per identity, written once, in a private
//! directory: the commit both submission-time audit records share
//! ([`super::file_link_audit`], [`super::turn_origin_audit`]).
//!
//! A record is published create-only. One already there must say the same
//! thing, apart from when each writer saw it, or the commit fails: a retry
//! reconciles with the evidence it left, and contradictory evidence for one
//! identity fails closed.
use crate::application::{ConversationError, ConversationFuture};
use nessa_local_storage::{open, sync_directory, OpenMode, PrivateTempFile};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

/// Commit `value` as the record `id` in `directory`, once: a record already
/// there must say the same thing, except for when each writer saw it, or the
/// commit fails. A stored record past `max_bytes` is treated as damaged.
pub(super) fn commit_evidence(
    directory: PathBuf,
    id: String,
    value: Value,
    max_bytes: usize,
) -> ConversationFuture<'static, ()> {
    Box::pin(async move {
        tokio::task::spawn_blocking(move || {
            let destination = directory.join(format!("{id}.json"));
            // Reading first is the common path and gives the better
            // failure — "this contradicts what is stored" rather than "the
            // name is taken". It is not what makes this safe: two
            // submissions of the same identity arriving together can both
            // read nothing, and it is `publish` below, which refuses a
            // destination that exists, that settles which one wrote.
            if let Stored::Agrees = stored_evidence(&destination, &value, &id, max_bytes)? {
                return sync_directory(&directory).map_err(audit("sync a directory"));
            }
            let mut file = PrivateTempFile::new_in(&directory).map_err(audit("open a record"))?;
            serde_json::to_writer(file.as_file_mut(), &value).map_err(|error| {
                tracing::error!(record = %id, %error, "could not write evidence");
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
                if error.kind() != ErrorKind::AlreadyExists {
                    tracing::error!(record = %id, %error, "could not publish a record");
                    return Err(ConversationError::Audit);
                }
                // Whatever beat this writer to the name has to say the
                // same thing. It having since been removed is not proof of
                // anything, so it is not treated as agreement.
                return match stored_evidence(&destination, &value, &id, max_bytes)? {
                    Stored::Agrees => sync_directory(&directory).map_err(audit("sync a directory")),
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

/// Why an audit write failed, said once where it happened. Every one of these
/// answers the caller with the same `Audit`, which is deliberate — a refused
/// submission should not depend on which syscall failed — but a full disk and
/// evidence that contradicts a submission are very different things to be
/// looking at afterwards, so they are not the same line in a log.
fn audit(doing: &'static str) -> impl Fn(io::Error) -> ConversationError {
    move |error| {
        tracing::error!(%error, "could not {doing} for durable evidence");
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
#[derive(Debug)]
pub(super) enum Stored {
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
pub(super) fn stored_evidence(
    destination: &Path,
    value: &Value,
    id: &str,
    max_bytes: usize,
) -> Result<Stored, ConversationError> {
    let mut file = match open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Stored::Absent),
        Err(error) => {
            tracing::error!(record = %id, %error, "could not read stored evidence");
            return Err(ConversationError::Audit);
        }
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(audit("read a record"))?;
    if bytes.len() > max_bytes {
        tracing::error!(record = %id, "a stored record is larger than any this writes");
        return Err(ConversationError::Audit);
    }
    let stored: Value = serde_json::from_slice(&bytes).map_err(|_| ConversationError::Audit)?;
    if comparable(stored) != comparable(value.clone()) {
        tracing::error!(
            record = %id,
            "stored evidence contradicts this submission"
        );
        return Err(ConversationError::Audit);
    }
    Ok(Stored::Agrees)
}

/// `value` with the field two writers may honestly differ on removed.
fn comparable(mut value: Value) -> Value {
    let _ = value
        .as_object_mut()
        .and_then(|object| object.remove(MAY_DIFFER_BETWEEN_WRITERS));
    value
}

pub(super) fn digest_of(bytes: &[u8]) -> String {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into()).to_hex()
}
