//! One part of a saved conversation this build could not fold.
//!
//! The bytes stay in the stream. The chat shows a placeholder at the part's
//! position and keeps reading every later record.

use super::StorageError;

/// Why a saved part could not be folded.
///
/// The variants match the tracing warning for that part. A later report can
/// name the same session, position, and reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnreadableReason {
    /// The record's `schemaVersion` is missing or is not this build's.
    AnotherVersion {
        /// The marker that was present, when it was an unsigned integer.
        found: Option<u64>,
    },
    /// The record belongs to another chat.
    Identity,
    /// The body is corrupt, too large, or contradicts the fold.
    Unreadable,
}

impl UnreadableReason {
    pub(crate) fn from_storage(error: &StorageError) -> Self {
        match error {
            StorageError::AnotherVersion { found } => Self::AnotherVersion { found: *found },
            StorageError::IdentityMismatch => Self::Identity,
            _ => Self::Unreadable,
        }
    }

    /// Stable name a protocol row and a tracing field share.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AnotherVersion { .. } => "another_version",
            Self::Identity => "identity",
            Self::Unreadable => "unreadable",
        }
    }

    /// The foreign `schemaVersion`, when [`Self::name`] is `another_version`.
    pub fn found(&self) -> Option<u64> {
        match self {
            Self::AnotherVersion { found } => *found,
            Self::Identity | Self::Unreadable => None,
        }
    }
}

/// A placeholder for one dropped save group.
///
/// `position` is the first physical record of that group. `after_invocation`
/// is how many folded invocations were already published when the group was
/// dropped, so a transcript can place the row there. The session id is the
/// conversation the record was read for; the projection adds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreadablePart {
    position: u64,
    after_invocation: u64,
    reason: UnreadableReason,
}

impl UnreadablePart {
    /// A placeholder for the save group that starts at `position`.
    ///
    /// `after_invocation` is how many folded invocations were already published
    /// when the group was dropped.
    pub fn new(position: u64, after_invocation: u64, reason: UnreadableReason) -> Self {
        Self {
            position,
            after_invocation,
            reason,
        }
    }

    /// First physical record of the dropped group.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Folded invocations already published when this group was dropped.
    pub fn after_invocation(&self) -> u64 {
        self.after_invocation
    }

    /// Why the group was dropped.
    pub fn reason(&self) -> UnreadableReason {
        self.reason
    }
}
