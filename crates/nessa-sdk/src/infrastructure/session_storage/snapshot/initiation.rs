//! JSON representation of who started a turn and whether its tools were checked.
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
enum Initiator {
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
            initiator: match value.initiator() {
                DomainInitiator::Owner => Initiator::Owner,
                DomainInitiator::Grant(grant) => Initiator::Grant {
                    grant: grant.grant().into(),
                    policy_revision: grant.policy_revision(),
                },
            },
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
        Ok(match self.initiator {
            Initiator::Owner => DomainInitiation::owner(),
            Initiator::Grant {
                grant,
                policy_revision,
            } => DomainInitiation::grant(
                GrantInitiator::new(grant, policy_revision).map_err(corrupt)?,
            ),
        })
    }
}
