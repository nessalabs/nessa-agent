//! JSON representation of who started a turn and whether its tools were
//! checked, and of the authority a note was left under (the same initiator).
//! Decoding goes back through the domain constructors, so a saved grant breaks
//! the same rules a live one would and is refused as corrupt.
use super::tools::corrupt;
use crate::application::agent_execution::sessions::StorageError;
use crate::domain::agent_execution::executions::{
    AgentSource, ExecutionId, GrantInitiator, Initiation as DomainInitiation,
    Initiator as DomainInitiator, ToolChecking as DomainToolChecking,
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
    Grant {
        grant: String,
        policy_revision: u64,
    },
    Agent {
        conversation: String,
        execution: String,
    },
}
#[derive(Serialize, Deserialize)]
enum ToolChecking {
    NotChecked,
    Checked,
}
impl From<&DomainInitiation> for Initiation {
    fn from(value: &DomainInitiation) -> Self {
        Self {
            initiator: value.initiator().into(),
            tools: match value.tools() {
                DomainToolChecking::NotChecked => ToolChecking::NotChecked,
                DomainToolChecking::Checked => ToolChecking::Checked,
            },
        }
    }
}
impl Initiation {
    pub(super) fn decode(self) -> Result<DomainInitiation, StorageError> {
        // Only a grant's turn is checked; an owner's turn saved as checked
        // is no turn the domain builds.
        Ok(match (self.initiator.decode()?, self.tools) {
            (DomainInitiator::Owner, ToolChecking::NotChecked) => DomainInitiation::owner(),
            (DomainInitiator::Owner, ToolChecking::Checked) => {
                return Err(corrupt("an owner's turn saved as tool-checked"))
            }
            (DomainInitiator::Grant(grant), ToolChecking::NotChecked) => {
                DomainInitiation::grant(grant)
            }
            (DomainInitiator::Grant(grant), ToolChecking::Checked) => {
                DomainInitiation::checked_grant(grant)
            }
            (DomainInitiator::Agent(source), ToolChecking::NotChecked) => {
                DomainInitiation::agent(source)
            }
            (DomainInitiator::Agent(_), ToolChecking::Checked) => {
                return Err(corrupt("an agent's turn saved as tool-checked"))
            }
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
            DomainInitiator::Agent(source) => Self::Agent {
                conversation: source.conversation().into(),
                execution: source.execution().as_str().into(),
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
            Self::Agent {
                conversation,
                execution,
            } => DomainInitiator::Agent(
                AgentSource::new(conversation, ExecutionId::new(execution).map_err(corrupt)?)
                    .map_err(corrupt)?,
            ),
        })
    }
}
