//! One stored representation of verified agent origin, shared by input and copied history.
use super::tools::corrupt;
use crate::{
    application::agent_execution::sessions::StorageError,
    domain::agent_execution::executions::{AgentSource, ExecutionId},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireAgentSource {
    conversation: String,
    execution: String,
}
impl From<&AgentSource> for WireAgentSource {
    fn from(source: &AgentSource) -> Self {
        Self {
            conversation: source.conversation().into(),
            execution: source.execution().as_str().into(),
        }
    }
}
impl WireAgentSource {
    pub(super) fn decode(self) -> Result<AgentSource, StorageError> {
        AgentSource::new(
            self.conversation,
            ExecutionId::new(self.execution).map_err(corrupt)?,
        )
        .map_err(corrupt)
    }
}
