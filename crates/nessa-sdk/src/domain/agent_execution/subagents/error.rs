//! Typed refusals for parent ownership. Callers match variants; display text is not a decision.
#![deny(missing_docs)]

use std::{error::Error, fmt};

/// Why an ownership transition was refused or could not be treated as dispatchable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipError {
    /// A required value contains no non-whitespace text.
    EmptyValue(&'static str),
    /// A constrained value exceeds its UTF-8 byte limit.
    ValueTooLong {
        /// Field whose value was rejected.
        field: &'static str,
        /// Maximum permitted UTF-8 byte length.
        max_bytes: usize,
    },
    /// An identity is not a portable key of letters, digits, underscores, and hyphens.
    InvalidIdentity(&'static str),
    /// A task digest is not 64 lowercase hexadecimal characters.
    InvalidTaskDigest,
    /// The named parent lifetime is not in this graph.
    ParentMissing,
    /// The parent lifetime is sealing or draining, so it admits no new child work.
    ParentClosing,
    /// The parent lifetime is sealed. A new lifetime is a different identity.
    ParentClosed,
    /// The child lifetime is closing or closed, so its initial task is not dispatched.
    ChildUnavailable,
    /// This spawn request id already names a different binding.
    RequestConflict,
    /// The child binding cannot honor the selected approval policy.
    UnsupportedPolicy,
    /// The parent's approval mode is still pending, so no policy can be copied.
    PolicyPending,
    /// The parent's approval mode has no committed result.
    PolicyUncertain,
    /// The live-conversation owner has no remaining slot.
    NoRoom,
    /// This parent already holds the maximum number of live direct children.
    DirectChildrenExceeded,
    /// Attaching here would pass the maximum tree depth.
    DepthExceeded,
    /// Retained spawn requests have reached the published limit.
    RetainedRequestsExceeded,
    /// A page size is outside the published read bound.
    PageLimit,
    /// The relationship graph contains a cycle.
    Cycle,
    /// A binding names a parent session that is not that lifetime's session.
    ForeignParent,
    /// Retained rows contradict each other. Dispatch stays refused and rows stay.
    Contradictory,
    /// Restored ownership refused dispatch. History was not rewritten.
    DispatchRefused,
    /// This session already has an open or closing root lifetime.
    LifetimeStillOpen,
    /// The spawn request is not an open reservation.
    UnknownSpawn,
    /// The transition does not apply to this spawn's current progress.
    IllegalSpawnProgress,
    /// A cleanup or lookup result names a different close or an unknown target.
    StaleOutcome,
    /// A child lifetime cannot be its own parent.
    SelfParent,
    /// This report id already belongs to a different child.
    ReportConflict,
    /// The child has no parent relationship to deliver through.
    UnknownChild,
}

impl fmt::Display for OwnershipError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue(field) => write!(output, "empty {field}"),
            Self::ValueTooLong { field, max_bytes } => {
                write!(output, "{field} exceeds {max_bytes} bytes")
            }
            Self::InvalidIdentity(field) => write!(output, "invalid {field}"),
            Self::InvalidTaskDigest => output.write_str("invalid task digest"),
            Self::ParentMissing => output.write_str("parent lifetime is missing"),
            Self::ParentClosing => output.write_str("parent lifetime is closing"),
            Self::ParentClosed => output.write_str("parent lifetime is closed"),
            Self::ChildUnavailable => output.write_str("child lifetime is not open"),
            Self::RequestConflict => output.write_str("spawn request conflicts with its binding"),
            Self::UnsupportedPolicy => output.write_str("inherited approval policy is unsupported"),
            Self::PolicyPending => output.write_str("parent approval mode is pending"),
            Self::PolicyUncertain => output.write_str("parent approval mode is uncertain"),
            Self::NoRoom => output.write_str("no live conversation room"),
            Self::DirectChildrenExceeded => output.write_str("direct child limit reached"),
            Self::DepthExceeded => output.write_str("subagent depth limit reached"),
            Self::RetainedRequestsExceeded => {
                output.write_str("retained spawn request limit reached")
            }
            Self::PageLimit => output.write_str("child page limit is outside the published bound"),
            Self::Cycle => output.write_str("ownership graph contains a cycle"),
            Self::ForeignParent => output.write_str("parent session does not match the lifetime"),
            Self::Contradictory => output.write_str("ownership records contradict each other"),
            Self::DispatchRefused => output.write_str("ownership dispatch is refused"),
            Self::LifetimeStillOpen => output.write_str("session already has an active lifetime"),
            Self::UnknownSpawn => output.write_str("unknown spawn request"),
            Self::IllegalSpawnProgress => {
                output.write_str("spawn progress does not allow this step")
            }
            Self::StaleOutcome => output.write_str("outcome does not match the open close"),
            Self::SelfParent => output.write_str("a lifetime cannot parent itself"),
            Self::ReportConflict => output.write_str("result report conflicts with its child"),
            Self::UnknownChild => output.write_str("unknown child lifetime"),
        }
    }
}

impl Error for OwnershipError {}
