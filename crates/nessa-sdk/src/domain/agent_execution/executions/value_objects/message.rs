#![deny(missing_docs)]
use crate::domain::agent_execution::ExecutionError;

/// Opaque provider identity shared by fragments of one streamed message.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageId(Box<str>);
impl MessageId {
    /// Maximum UTF-8 byte length retained across adapters and storage.
    pub const MAX_BYTES: usize = 256;
    /// Preserve exact nonempty `value` up to [`Self::MAX_BYTES`].
    ///
    /// Returns [`ExecutionError::EmptyValue`] for an empty identity and
    /// [`ExecutionError::ValueTooLong`] when the UTF-8 representation is too long.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ExecutionError::EmptyValue("message ID"));
        }
        if value.len() > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "message ID",
                max_bytes: Self::MAX_BYTES,
            });
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// Borrow the exact provider identity without normalization.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which provider output channel an immutable message fragment belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    /// User-visible output.
    Text,
    /// Reasoning explicitly exposed by the provider.
    Thought,
}

/// Immutable incremental output, separate from a complete persisted message.
/// Empty text and exact whitespace are preserved. Compact storage discards caller
/// spare capacity; application admission applies its own payload-byte limit.
///
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::executions::MessageChunk;
/// let mut chunk = MessageChunk::text("original");
/// chunk.text = "replacement".into();
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageChunk {
    kind: MessageKind,
    message_id: Option<MessageId>,
    text: Box<str>,
}
impl MessageChunk {
    /// Own user-visible `text`, preserving empty text and whitespace exactly.
    /// Construction is infallible and compacts spare capacity without imposing a
    /// provider or application size policy. Changes create a replacement value.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            message_id: None,
            kind: MessageKind::Text,
            text: text.into().into_boxed_str(),
        }
    }
    /// Own provider-exposed reasoning `text`, preserving its exact contents.
    /// Construction is infallible, compacts spare capacity, and imposes no size policy.
    pub fn thought(text: impl Into<String>) -> Self {
        Self {
            message_id: None,
            kind: MessageKind::Thought,
            text: text.into().into_boxed_str(),
        }
    }
    /// Retain a validated opaque provider message identity across streamed fragments.
    pub fn with_message_id(mut self, id: MessageId) -> Self {
        self.message_id = Some(id);
        self
    }
    /// Provider message identity, when the transport exposes one.
    pub fn message_id(&self) -> Option<&str> {
        self.message_id.as_ref().map(MessageId::as_str)
    }
    /// Output channel associated with this exact fragment.
    pub fn kind(&self) -> MessageKind {
        self.kind
    }
    /// Borrow exact text without granting mutation authority.
    pub fn as_str(&self) -> &str {
        &self.text
    }
    /// Retained UTF-8 payload bytes, excluding this value's fixed struct size.
    /// This is the exact text byte length plus the optional message ID byte length;
    /// both allocations are compact and retain no caller spare capacity.
    pub fn payload_bytes(&self) -> usize {
        self.text.len() + self.message_id.as_ref().map_or(0, |id| id.as_str().len())
    }
    /// Consume the fragment and transfer its exact text allocation to the caller.
    /// The caller can inspect `kind()` before consumption when the channel matters.
    pub fn into_text(self) -> String {
        self.text.into_string()
    }
}

/// Terminal outcome reported by an execution, independent of transport failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionOutcome {
    /// The execution completed normally.
    Completed,
    /// The execution reached its output token limit.
    OutputLimit,
    /// The provider stopped at its request or turn limit.
    RequestLimit,
    /// The provider declined the request.
    Refused,
    /// The execution stopped after cancellation.
    Cancelled,
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/message_storage.rs"]
mod storage_tests;
