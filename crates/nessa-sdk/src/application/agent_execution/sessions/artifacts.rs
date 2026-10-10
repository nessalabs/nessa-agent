//! Artifact records in a conversation's stream: a file an agent published
//! under a lease, collected and held by the conversation.
//!
//! ```text
//! ArtifactRecord ──admit(snapshot)──▶ appended to SessionSnapshot::artifacts
//!                       │
//!                       └─▶ ArtifactRefusal ──▶ ArtifactRecordError (commit)
//!                                           └──▶ StorageError::Corrupt (read back)
//! ```
//!
//! Arrows are the one rule both paths ask: a record is committed only while
//! the lease it names is the conversation's latest, still live, and issued by
//! the actor it names, and the same rule reads it back. The list only grows; a
//! record past [`ArtifactRecord::MAX_PER_CONVERSATION`] is refused, never
//! made room for. An artifact recorded under a lease this build cannot read is
//! kept as written, as that lease's own records are.
use super::{CurrentLeaseState, LeaseRecord, SessionSnapshot, StorageError};
use crate::application::agent_execution::permissions::ActionContext;
use crate::domain::agent_execution::{
    executions::ExecutionId,
    leases::{LeaseId, LeasePhase},
};
use crate::domain::common::value_objects::{MediaType, Sha256Digest};
use std::{collections::HashSet, error::Error, fmt};

/// A value of an artifact record is outside what one may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactValueError {
    /// The name is empty, longer than [`ArtifactName::MAX_BYTES`], or holds a
    /// `/` or a control character.
    Name,
    /// The size is zero or larger than [`PublishedFile::MAX_BYTES`].
    Size,
}
impl fmt::Display for ArtifactValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name => write!(
                f,
                "an artifact name must be 1 to {} bytes with no `/` or control character",
                ArtifactName::MAX_BYTES
            ),
            Self::Size => write!(
                f,
                "an artifact must be 1 to {} bytes long",
                PublishedFile::MAX_BYTES
            ),
        }
    }
}
impl Error for ArtifactValueError {}

/// The file name an artifact was published under, for people to read only:
/// it names no place, so it holds no `/`, and it is shown on one line, so it
/// holds no control character (NUL among them). At most
/// [`Self::MAX_BYTES`] of UTF-8.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArtifactName(Box<str>);
impl ArtifactName {
    /// Longest name, in bytes: a file name's bound.
    pub const MAX_BYTES: usize = 255;

    /// Accept `value` when it is a nonempty name of at most
    /// [`Self::MAX_BYTES`] with no `/` and no control character; otherwise
    /// [`ArtifactValueError::Name`].
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactValueError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_BYTES
            || value.chars().any(|c| c == '/' || c.is_control())
        {
            return Err(ArtifactValueError::Name);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The name as given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a published file is: the digest of its bytes, what it was declared
/// as, and its length. It names the bytes; it does not hold them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedFile {
    digest: Sha256Digest,
    media_type: MediaType,
    size: u64,
}
impl PublishedFile {
    /// Most bytes one published file may have: the most one attachment may
    /// hold, since the conversation keeps the file as one.
    pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

    /// Accept a file of 1 to [`Self::MAX_BYTES`] bytes; otherwise
    /// [`ArtifactValueError::Size`]. An empty file is nothing to publish.
    pub fn new(
        digest: Sha256Digest,
        media_type: MediaType,
        size: u64,
    ) -> Result<Self, ArtifactValueError> {
        if size == 0 || size > Self::MAX_BYTES {
            return Err(ArtifactValueError::Size);
        }
        Ok(Self {
            digest,
            media_type,
            size,
        })
    }
    /// The SHA-256 of its bytes.
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    /// What it was declared as.
    pub fn media_type(&self) -> &MediaType {
        &self.media_type
    }
    /// Its length in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// One artifact the conversation recorded: a file published under `lease`,
/// read, verified by its digest and kept. Committed to the conversation's
/// stream after the file is held, never before, and kept as history when the
/// hold is later let go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRecord {
    /// The lease it was published under: the conversation's latest when it
    /// was recorded, and live then.
    pub lease: LeaseId,
    /// The turn that was running when it was recorded, if one was. It names
    /// a turn the conversation accepted.
    pub turn: Option<ExecutionId>,
    /// The name it was published under, for people only.
    pub name: ArtifactName,
    /// What the file is.
    pub file: PublishedFile,
    /// Who asked for the work that published it: the lease's issuer.
    pub actor: ActionContext,
}
impl ArtifactRecord {
    /// Most artifacts one conversation records. Another is refused rather
    /// than an earlier one dropped.
    pub const MAX_PER_CONVERSATION: usize = 1024;

    /// Whether `self` may follow what `snapshot` already records, `knows_turn`
    /// saying whether the conversation accepted a turn. One rule for
    /// committing a record and reading it back.
    pub(super) fn admit(
        &self,
        snapshot: &SessionSnapshot,
        knows_turn: impl Fn(&ExecutionId) -> bool,
    ) -> Result<(), ArtifactRefusal> {
        let current = snapshot
            .lease
            .as_ref()
            .ok_or(ArtifactRefusal::NotThisLease)?;
        match current.state() {
            CurrentLeaseState::Unreadable { .. } => return Err(ArtifactRefusal::LeaseUnreadable),
            CurrentLeaseState::Refused { .. } => return Err(ArtifactRefusal::NotThisLease),
            CurrentLeaseState::Held(lease)
                if lease.id() != &self.lease
                    || lease.phase() != LeasePhase::Live
                    || current.issued_by() != Some(&self.actor) =>
            {
                return Err(ArtifactRefusal::NotThisLease);
            }
            CurrentLeaseState::Held(_) => {}
        }
        if self.turn.as_ref().is_some_and(|turn| !knows_turn(turn)) {
            return Err(ArtifactRefusal::UnknownTurn);
        }
        // Last, so a refusal for room is one the record would otherwise
        // pass: what lets a file already recorded be answered as recorded.
        if snapshot.artifacts.len() >= Self::MAX_PER_CONVERSATION {
            return Err(ArtifactRefusal::Full);
        }
        Ok(())
    }

    /// As [`Self::admit`], for a record read back: a refusal is corrupt
    /// evidence, except under a lease this build cannot read, whose records
    /// cannot be checked and are kept as written.
    pub(super) fn admit_saved(
        &self,
        snapshot: &SessionSnapshot,
        knows_turn: impl Fn(&ExecutionId) -> bool,
    ) -> Result<(), StorageError> {
        // The bound holds under every lease, one this build cannot read too.
        if snapshot.artifacts.len() >= Self::MAX_PER_CONVERSATION {
            return Err(StorageError::Corrupt(format!(
                "artifact record: {}",
                ArtifactRefusal::Full
            )));
        }
        match self.admit(snapshot, knows_turn) {
            Ok(()) | Err(ArtifactRefusal::LeaseUnreadable) => Ok(()),
            Err(refusal) => Err(StorageError::Corrupt(format!("artifact record: {refusal}"))),
        }
    }
}

/// Why [`ArtifactRecord::admit`] refused a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArtifactRefusal {
    /// The conversation already records its most artifacts.
    Full,
    /// The record's lease is not the conversation's latest, is not live, or
    /// was not issued by the record's actor.
    NotThisLease,
    /// The conversation's latest lease is of a kind this build cannot read.
    LeaseUnreadable,
    /// The record names a turn the conversation did not accept.
    UnknownTurn,
}
impl fmt::Display for ArtifactRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Full => "the conversation records its most artifacts",
            Self::NotThisLease => "it names a lease that is not the latest live one",
            Self::LeaseUnreadable => "the latest lease cannot be read",
            Self::UnknownTurn => "it names a turn the conversation did not accept",
        })
    }
}

/// Whether every artifact `snapshot` saves stays within the conversation's
/// bound, names a turn it accepted, and, when it names the snapshot's latest
/// lease, was published under it as [`ArtifactRecord::admit`] allows: that
/// lease was issued, to the record's actor. A saved snapshot keeps only the
/// latest lease, so an artifact under an earlier one is kept as written; a
/// record log replays each beside its lease and checks it in full.
pub(super) fn validate_saved(snapshot: &SessionSnapshot) -> Result<(), StorageError> {
    if snapshot.artifacts.len() > ArtifactRecord::MAX_PER_CONVERSATION {
        return Err(StorageError::Corrupt(
            "artifact record: more than one conversation records".into(),
        ));
    }
    let accepted: HashSet<&ExecutionId> = snapshot
        .invocations
        .iter()
        .map(|invocation| &invocation.request.execution_id)
        .collect();
    let named_turn_unknown = snapshot.artifacts.iter().any(|artifact| {
        artifact
            .turn
            .as_ref()
            .is_some_and(|turn| !accepted.contains(turn))
    });
    if named_turn_unknown {
        return Err(StorageError::Corrupt(format!(
            "artifact record: {}",
            ArtifactRefusal::UnknownTurn
        )));
    }
    let Some(latest) = snapshot.lease.as_ref() else {
        return Ok(());
    };
    // Who the latest lease was issued to, or `None` when it was refused and
    // so published nothing. One this build cannot read is not checked.
    let (lease, issuer) = match latest.state() {
        CurrentLeaseState::Unreadable { .. } => return Ok(()),
        CurrentLeaseState::Held(lease) => (lease.id(), latest.issued_by()),
        CurrentLeaseState::Refused { .. } => match latest.records().first() {
            Some(LeaseRecord::Refused { lease, .. }) => (lease, None),
            _ => return Ok(()),
        },
    };
    let unproven = snapshot
        .artifacts
        .iter()
        .any(|artifact| &artifact.lease == lease && issuer != Some(&artifact.actor));
    if unproven {
        return Err(StorageError::Corrupt(format!(
            "artifact record: {}",
            ArtifactRefusal::NotThisLease
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/artifacts.rs"]
mod tests;
