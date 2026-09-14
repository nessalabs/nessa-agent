#![deny(missing_docs)]

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
    text: Box<str>,
}
impl MessageChunk {
    /// Own user-visible `text`, preserving empty text and whitespace exactly.
    /// Construction is infallible and compacts spare capacity without imposing a
    /// provider or application size policy. Changes create a replacement value.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: MessageKind::Text,
            text: text.into().into_boxed_str(),
        }
    }
    /// Own provider-exposed reasoning `text`, preserving its exact contents.
    /// Construction is infallible, compacts spare capacity, and imposes no size policy.
    pub fn thought(text: impl Into<String>) -> Self {
        Self {
            kind: MessageKind::Thought,
            text: text.into().into_boxed_str(),
        }
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
    /// Compact immutable storage makes this equal to the text byte length.
    pub fn payload_bytes(&self) -> usize {
        self.text.len()
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
