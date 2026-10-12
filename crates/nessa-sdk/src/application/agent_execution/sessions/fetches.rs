//! What a fetch under an agent lease is recorded as: where it was asked to
//! go, and what it came to.
//!
//! ```text
//! LeaseRecord::FetchAsked{destination} ──▶ the environment asked
//!   ──▶ LeaseRecord::FetchEnded{FetchOutcome}
//!         placed{path} | alreadyPlaced{path} | refused{FetchRefusalReason}
//! ```
//!
//! Arrows are one fetch, in order. The values here only name what was asked
//! and answered; [`super::CurrentLease`] owns which fetch records may follow
//! which.
use super::ArtifactValueError;

/// A path in an environment's file system, as text: a fetch's destination
/// as the agent asked for it, absolute or relative to the workspace, or
/// where the environment placed the file. Nonempty and at most
/// [`Self::MAX_BYTES`]; nothing else about it is read here, since only the
/// environment can say what it names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnvironmentPath(Box<str>);
impl EnvironmentPath {
    /// Longest path, in bytes: the most an environment reads or answers.
    pub const MAX_BYTES: usize = 1024;

    /// Accept `value` when it is 1 to [`Self::MAX_BYTES`] bytes; otherwise
    /// [`ArtifactValueError::Path`].
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactValueError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_BYTES {
            return Err(ArtifactValueError::Path);
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The path as given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a fetch came to, as its end records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The environment placed the file at `path`, where nothing was before.
    Placed {
        /// Where it is, as the environment resolved it.
        path: EnvironmentPath,
    },
    /// `path` already was exactly this file: nothing was written.
    AlreadyPlaced {
        /// Where it is, as the environment resolved it.
        path: EnvironmentPath,
    },
    /// It is not placed, or not known to be, with why.
    Refused(FetchRefusalReason),
}

/// Why a fetch did not place its file, or does not know whether it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchRefusalReason {
    /// The environment did not place it, with its own reason.
    Environment(EnvironmentFetchRefusal),
    /// The conversation does not hold exactly that file for the person who
    /// asked, or let go of it while it was written. Nothing is placed.
    NotHeld,
    /// The file's bytes could not be read where the conversation keeps it.
    /// Nothing is placed.
    SourceUnavailable,
    /// The bytes could not be written to the environment. Nothing is placed.
    ChannelUnavailable,
    /// No answer came. `delivered` says whether every byte reached the
    /// environment first: only then may it be placed, and whether it is is
    /// not known; otherwise nothing is placed.
    Unanswered {
        /// Whether every byte was delivered before the answer was lost.
        delivered: bool,
    },
    /// Not answered within the call's wait. `delivered` says whether every
    /// byte reached the environment first: only then may it still be
    /// placed. Not delivered covers an environment never asked at all.
    TimedOut {
        /// Whether every byte was delivered before the wait ran out.
        delivered: bool,
    },
    /// The call was cancelled while it waited. `delivered` says whether
    /// every byte reached the environment first: only then may it still be
    /// placed. Not delivered covers an environment never asked at all.
    Cancelled {
        /// Whether every byte was delivered before the call was cancelled.
        delivered: bool,
    },
    /// Its asking could not be saved, so the environment was never asked.
    /// Nothing is placed.
    AuditUnavailable,
}

/// An environment's own reason for not placing a fetched file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvironmentFetchRefusal {
    /// The environment takes no files.
    Unavailable,
    /// The destination is not a place inside the workspace.
    OutsideWorkspace,
    /// The destination's directory does not exist.
    NoDirectory,
    /// Something other than exactly this file is at the destination.
    Occupied,
    /// The file is empty or larger than a file may be.
    Size,
    /// The environment could not read what it was asked.
    Invalid,
    /// The lease already has as many fetches in flight as it may.
    Busy,
    /// The bytes delivered are not the file asked for.
    Mismatch,
    /// The file could not be made, read or linked in place.
    PlacingFailed,
    /// The environment could not record the fetch in its own audit.
    AuditUnavailable,
    /// The lease ended, or is not held there, before the file was placed.
    LeaseEnded,
}
