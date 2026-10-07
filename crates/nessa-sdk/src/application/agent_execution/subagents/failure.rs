//! Failures returned by the ownership coordinator.
#![deny(missing_docs)]

use std::{error::Error, fmt};

use super::ports::PortFailure;
use crate::domain::agent_execution::subagents::OwnershipError;

/// Why spawn, close, or recovery did not finish as an audited success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipFailure {
    /// The domain refused the transition.
    Domain(OwnershipError),
    /// Audit did not acknowledge the transition.
    Audit(PortFailure),
    /// Ownership storage did not acknowledge the snapshot.
    Store(PortFailure),
    /// The child factory failed before dispatch.
    Startup(PortFailure),
    /// The initial task submission failed or was lost.
    Submission(PortFailure),
    /// The delegated task was empty.
    EmptyTask,
    /// The named parent has no acknowledged admission or conservative retained identity.
    /// Read visibility does not authorize creation beneath a private parent.
    UnpublishedParent,
    /// Cleanup is still owned. The drain was not dropped.
    Incomplete,
}

impl fmt::Display for OwnershipFailure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => write!(output, "{error}"),
            Self::Audit(PortFailure::Rejected) => output.write_str("ownership audit rejected"),
            Self::Audit(PortFailure::Uncertain) => output.write_str("ownership audit is uncertain"),
            Self::Store(PortFailure::Rejected) => output.write_str("ownership store rejected"),
            Self::Store(PortFailure::Uncertain) => output.write_str("ownership store is uncertain"),
            Self::Startup(PortFailure::Rejected) => output.write_str("child startup rejected"),
            Self::Startup(PortFailure::Uncertain) => output.write_str("child startup is uncertain"),
            Self::Submission(PortFailure::Rejected) => {
                output.write_str("child submission rejected")
            }
            Self::Submission(PortFailure::Uncertain) => {
                output.write_str("child submission is uncertain")
            }
            Self::EmptyTask => output.write_str("delegated task is empty"),
            Self::UnpublishedParent => {
                output.write_str("parent ownership admission is not acknowledged")
            }
            Self::Incomplete => output.write_str("ownership close is incomplete"),
        }
    }
}

impl Error for OwnershipFailure {}
