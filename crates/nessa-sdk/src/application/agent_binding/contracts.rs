use crate::domain::agent_execution::value_objects::{
    FileToolInput, MessageChunk, PromptOutcome, ToolCallUpdate,
};
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use std::{error::Error, fmt, future::Future, pin::Pin, sync::Arc};

pub type BindingFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BindingError>> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingError {
    Configuration(String),
    Unsupported(String),
    InvalidInput(String),
    Busy,
    Closed,
    StalePermission,
    Protocol(String),
    Provider { code: i64 },
    Transport(String),
    Deadline,
    Backpressure,
    CleanupUncertain,
}
impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent binding: {self:?}")
    }
}
impl Error for BindingError {}

/// Counts include existing context and tool material, supplied by the caller.
/// The budget validates admission; it is not a provider usage measurement.
#[derive(Clone, Debug)]
pub struct Prompt {
    /// Correlation supplied by the host; this port does not implement deduplication.
    pub execution_id: String,
    pub text: String,
    pub input_tokens: u64,
    pub reserved_output_tokens: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingEvent {
    pub execution_id: String,
    pub update: BindingUpdate,
}
/// One provider-independent observation carried by a BindingEvent.
/// Adapters translate provider messages into these variants; consumers render or
/// record them. This enum does not execute tools, decide permissions, or own
/// Conversation state. Finished orders the terminal observation after its output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingUpdate {
    Finished(Result<PromptOutcome, BindingError>),
    Message(MessageChunk),
    Tool(ToolCallUpdate),
    PermissionRequested {
        id: String,
        tool: ToolCallUpdate,
        input: Box<FileToolInput>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionAnswer {
    pub execution_id: String,
    pub id: String,
    pub allow_once: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopOutcome {
    pub forced: bool,
}

/// One provider context. Methods may run concurrently; only one prompt may run.
/// Stop permanently closes this binding and resolves only after verified cleanup.
/// Dropping all handles requests shutdown; hosts must await Stop before exit.
pub trait AgentSession: Send + Sync {
    fn prompt(&self, input: Prompt) -> BindingFuture<'_, PromptOutcome>;
    fn answer_permission(&self, answer: PermissionAnswer) -> BindingFuture<'_, ()>;
    fn stop(&self) -> BindingFuture<'_, StopOutcome>;
}
/// A single reader. Exhaustion is not a successful prompt outcome; await prompt.
pub trait BindingEvents: Send {
    fn next(&mut self) -> BindingFuture<'_, Option<BindingEvent>>;
}
pub struct OpenedBinding {
    pub session: Arc<dyn AgentSession>,
    pub events: Box<dyn BindingEvents>,
    pub capabilities: EffectiveCapabilities,
}
/// Composition constructs a factory with an immutable model and execution profile.
pub trait AgentBinding: Send + Sync {
    fn open(&self) -> BindingFuture<'_, OpenedBinding>;
}
