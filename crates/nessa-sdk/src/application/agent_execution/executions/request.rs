use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::{executions::ExecutionId, prompts::UserMessage};

/// One new message with caller-estimated context use and a requested output budget.
/// Provider usage measurements and configured ceilings remain separate values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionRequest {
    /// Stable submission key. Agent queue/steering retries with the same key recover
    /// the original delivery; changing the input or attribution is a conflict.
    pub execution_id: ExecutionId,
    /// What the user said: text of at most [`Self::MAX_MESSAGE_BYTES`] UTF-8 bytes,
    /// images referred to by digest, or both. Image bytes are never held here,
    /// so a request can be compared for retry identity and persisted whole; the
    /// adapter resolves them when it dispatches. Previously submitted messages
    /// are not replayed here.
    pub user_message: UserMessage,
    /// Caller estimate of total input context tokens, including retained history
    /// and tool material. Used only for local admission; ACP does not supply exact
    /// hidden context occupancy. This is neither a tokenizer result nor measured usage.
    pub estimated_input_tokens: u64,
    /// Output tokens reserved for this invocation. Estimate plus reservation must
    /// fit the effective context window; provider-specific constraints may narrow it.
    pub reserved_output_tokens: u32,
}

impl ExecutionRequest {
    /// Maximum new user-message text: 4 MiB of actual UTF-8 text, independent
    /// of a caller's token estimate. This bounds each admitted message before
    /// snapshot cloning or persistence; it is not a tokenizer or total-history cap.
    /// Provider framing and model context limits may be lower.
    pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

    pub(crate) fn validate_message_size(&self) -> Result<(), AgentError> {
        if self.user_message.text_str().len() > Self::MAX_MESSAGE_BYTES {
            return Err(AgentError::InvalidInput(
                "user message exceeds the 4 MiB byte limit".into(),
            ));
        }
        Ok(())
    }
}
