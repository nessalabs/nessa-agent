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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    Completed,
    OutputLimit,
    RequestLimit,
    Refused,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Search,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLocation {
    pub path: String,
    pub line: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub locations: Option<Vec<FileLocation>>,
    pub content: Option<Vec<ToolContent>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    Diff {
        path: String,
        old: Option<String>,
        new: String,
    },
}

/// Normalized file-tool inputs for a host to review before allowing an action.
/// Paths and contents are untrusted provider data, never authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileToolInput {
    Read {
        path: String,
        offset: Option<u64>,
        limit: Option<u64>,
        pages: Option<String>,
    },
    Write {
        path: String,
        content: String,
    },
    Edit {
        path: String,
        old: String,
        new: String,
        replace_all: bool,
    },
    Glob {
        pattern: String,
        path: Option<String>,
    },
    Grep {
        pattern: String,
        path: Option<String>,
        glob: Option<String>,
        file_type: Option<String>,
        output_mode: Option<String>,
        before: Option<u64>,
        after: Option<u64>,
        context: Option<u64>,
        line_numbers: Option<bool>,
        ignore_case: Option<bool>,
        only_matching: Option<bool>,
        multiline: Option<bool>,
        head_limit: Option<u64>,
        offset: Option<u64>,
    },
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
    Text(String),
    Thought(String),
    Tool(ToolCall),
    PermissionRequested {
        id: String,
        tool: ToolCall,
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
