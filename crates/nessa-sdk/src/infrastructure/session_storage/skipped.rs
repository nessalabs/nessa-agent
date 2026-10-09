//! A session record this build does not apply.
//!
//! The codec still refuses an unmarked record, another format version, and a
//! corrupt body. Readers skip that refusal, keep the records they can read,
//! and leave the stored bytes where they are. There is no migration.

#![deny(missing_docs)]

use crate::application::agent_execution::sessions::StorageError;

/// Why one saved session record was not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The record's `schemaVersion` is absent or is another unsigned integer.
    ///
    /// `found` is that integer, or `None` when the field is absent.
    AnotherVersion {
        /// The `schemaVersion` integer in the record, if it has one.
        found: Option<u64>,
    },
    /// The body cannot be decoded. The marker is this build's version, or it
    /// is not an unsigned integer. A decoded unit that the session lifecycle
    /// refuses is not this case.
    Corrupt,
}

/// One physical record the reader consumed and did not apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkippedRecord {
    position: u64,
    reason: SkipReason,
}

impl SkippedRecord {
    pub(crate) fn new(position: u64, reason: SkipReason) -> Self {
        Self { position, reason }
    }

    /// Physical position of the frame that completed the skipped unit.
    pub fn position(self) -> u64 {
        self.position
    }

    /// Why the unit was not applied.
    pub fn reason(self) -> SkipReason {
        self.reason
    }
}

impl SkipReason {
    /// Classifies a storage error as a record a reader skips.
    ///
    /// `None` means the failure still refuses the operation. Unmarked records,
    /// other format versions, and corrupt bodies are skipped. There is no
    /// migration and no reader for those bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use nessa_sdk::application::agent_execution::sessions::StorageError;
    /// use nessa_sdk::infrastructure::session_storage::SkipReason;
    ///
    /// assert_eq!(
    ///     SkipReason::classify(&StorageError::AnotherVersion { found: None }),
    ///     Some(SkipReason::AnotherVersion { found: None })
    /// );
    /// assert!(SkipReason::classify(&StorageError::Busy).is_none());
    /// ```
    pub fn classify(error: &StorageError) -> Option<Self> {
        match error {
            StorageError::AnotherVersion { found } => Some(Self::AnotherVersion { found: *found }),
            StorageError::Corrupt(_) => Some(Self::Corrupt),
            _ => None,
        }
    }
}

pub(crate) fn warn_skipped(session: &str, position: u64, reason: SkipReason) {
    match reason {
        SkipReason::AnotherVersion { found: Some(found) } => {
            tracing::warn!(
                session,
                position,
                reason = "another_version",
                found,
                "skipped a session record this build cannot read"
            );
        }
        SkipReason::AnotherVersion { found: None } => {
            tracing::warn!(
                session,
                position,
                reason = "another_version",
                "skipped a session record this build cannot read"
            );
        }
        SkipReason::Corrupt => {
            tracing::warn!(
                session,
                position,
                reason = "corrupt",
                "skipped a session record this build cannot read"
            );
        }
    }
}
