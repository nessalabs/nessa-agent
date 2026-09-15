use crate::application::agent_execution::agents::AgentError;

pub(crate) fn protocol(message: &str) -> AgentError {
    AgentError::Protocol(message.into())
}
