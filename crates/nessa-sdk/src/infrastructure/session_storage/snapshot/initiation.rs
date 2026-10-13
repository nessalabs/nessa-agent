//! JSON representation of who started a turn and whether its tools were
//! checked, and of the authority a note was left under (the same initiator).
//! Decoding goes back through the domain constructors, so a saved grant breaks
//! the same rules a live one would and is refused as corrupt.
use super::tools::corrupt;
use crate::application::agent_execution::sessions::StorageError;
use crate::domain::agent_execution::executions::{
    GrantInitiator, Initiation as DomainInitiation, Initiator as DomainInitiator,
    ToolChecking as DomainToolChecking,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Initiation {
    initiator: Initiator,
    tools: ToolChecking,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Initiator {
    Owner,
    Grant { grant: String, policy_revision: u64 },
}
#[derive(Serialize, Deserialize)]
enum ToolChecking {
    NotChecked,
}
impl From<&DomainInitiation> for Initiation {
    fn from(value: &DomainInitiation) -> Self {
        Self {
            initiator: value.initiator().into(),
            tools: match value.tools() {
                DomainToolChecking::NotChecked => ToolChecking::NotChecked,
            },
        }
    }
}
impl Initiation {
    pub(super) fn decode(self) -> Result<DomainInitiation, StorageError> {
        // One tool-checking case exists, and both constructors give it.
        let ToolChecking::NotChecked = self.tools;
        Ok(match self.initiator.decode()? {
            DomainInitiator::Owner => DomainInitiation::owner(),
            DomainInitiator::Grant(grant) => DomainInitiation::grant(grant),
        })
    }
}
impl From<&DomainInitiator> for Initiator {
    fn from(value: &DomainInitiator) -> Self {
        match value {
            DomainInitiator::Owner => Self::Owner,
            DomainInitiator::Grant(grant) => Self::Grant {
                grant: grant.grant().into(),
                policy_revision: grant.policy_revision(),
            },
        }
    }
}
impl Initiator {
    pub(super) fn decode(self) -> Result<DomainInitiator, StorageError> {
        Ok(match self {
            Self::Owner => DomainInitiator::Owner,
            Self::Grant {
                grant,
                policy_revision,
            } => DomainInitiator::Grant(
                GrantInitiator::new(grant, policy_revision).map_err(corrupt)?,
            ),
        })
    }
}
