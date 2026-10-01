//! Select allocation budgets from storage field roles, without replacing schema validation.
use crate::application::agent_execution::agents::{DiagnosticTreeLimits, ProviderDiagnostic};
use crate::application::agent_execution::executions::{
    limits::{MAX_MESSAGE_CHUNK_BYTES, MAX_RETAINED_OUTPUT_EVENTS},
    ExecutionRequest,
};
use crate::application::agent_execution::sessions::{QueueHistoryRecord, SessionSnapshot};
use crate::domain::agent_execution::{
    executions::QueueOrderChange,
    permissions::PermissionOption,
    prompts::{LinkedFile, UserMessage},
    questions::{
        MAX_KEY_BYTES as MAX_QUESTION_KEY_BYTES, MAX_OPTIONS as MAX_QUESTION_OPTIONS,
        MAX_QUESTIONS, MAX_TEXT_BYTES as MAX_QUESTION_TEXT_BYTES,
    },
    tools::{FileLocation, ToolContent, MAX_MCP_NAME_BYTES, MAX_STRUCTURED_RESULT_BYTES},
};
use std::mem::size_of;
use Shape::*;

pub(super) const LARGE_STRING: usize = 32 * 1024 * 1024;
pub(super) const ERROR_BYTES: usize = DiagnosticTreeLimits::BYTES;
pub(super) const KEY_BYTES: usize = 128;
/// `sha256:` and 64 hexadecimal digits: the one text form of a digest.
const DIGEST_BYTES: usize = 71;
/// Longer than any image media type the domain names (`image/jpeg` is ten).
const MEDIA_TYPE_BYTES: usize = 16;

#[derive(Clone, Copy)]
pub(super) enum Shape {
    Checkpoint,
    Snapshot,
    Invocations,
    Invocation,
    Events,
    QueueHistory,
    Semantic,
    SemanticBatch,
    SemanticChanges,
    SemanticOpened,
    SemanticInput,
    SemanticScheduling,
    SemanticReceipt,
    SemanticStop,
    SemanticReport,
    SemanticSettlement,
    SemanticContext,
    Metadata,
    Images,
    Image,
    Files,
    FileLink,
    Provider,
    Actor,
    Event,
    Update,
    Scheduling,
    FinalizedComponents,
    FinalizedComponent,
    Acknowledgement,
    FailedAcknowledgement,
    StorageError,
    StorageChildren,
    StorageDiagnostic,
    Reorder,
    QueueEntries,
    QueueEntry,
    QueueIds,
    Tool,
    McpTool,
    Review,
    Decline,
    Ask,
    AskedQuestions,
    Asked,
    AskedOptions,
    AskedOption,
    Content,
    ContentItem,
    Locations,
    Options,
    Generic,
    Result,
    Error,
    ProviderError,
    ErrorBody(bool),
    Hooks,
    Hook,
    Text(usize),
}
impl Shape {
    pub(super) fn string_limit(self) -> usize {
        match self {
            Self::Text(limit) => limit,
            Self::StorageDiagnostic => ERROR_BYTES,
            Self::Acknowledgement | Self::StorageError | Self::Error => KEY_BYTES,
            _ => LARGE_STRING,
        }
    }
    pub(super) fn field(self, key: &str) -> Self {
        match (self, key) {
            (Checkpoint, "snapshot") => Snapshot,
            (
                Checkpoint,
                "receiver" | "origin" | "stream" | "incarnation" | "schema" | "access_epoch",
            ) => Text(256),
            (Snapshot, "id" | "context") => Text(256),
            (Snapshot, "provider") => Provider,
            (Snapshot, "invocations") => Invocations,
            (Snapshot, "queue_history") => QueueHistory,
            (Invocation, "metadata") => Metadata,
            (Invocation, "scheduling") => Scheduling,
            (Invocation, "events") => Events,
            (SemanticBatch, "changes") => SemanticChanges,
            (Semantic, "Opened") => SemanticOpened,
            (Semantic, "InputAccepted") => SemanticInput,
            (Semantic, "QueueDecision") => Generic,
            (Semantic, "SchedulingTransition") => SemanticScheduling,
            (Semantic, "ProviderObservation") => Event,
            (Semantic, "ReceiptUpdated") => SemanticReceipt,
            (Semantic, "StopDecision") => SemanticStop,
            (Semantic, "ProviderReport") => SemanticReport,
            (Semantic, "LocalSettlement") => SemanticSettlement,
            (Semantic, "ProviderContext") => SemanticContext,
            (SemanticOpened, "id" | "context") => Text(256),
            (SemanticOpened, "provider") => Provider,
            (SemanticInput, "metadata") => Metadata,
            (SemanticInput, "scheduling") => Scheduling,
            (SemanticScheduling, "execution_id") => Text(256),
            (SemanticScheduling, "event") => Generic,
            (SemanticReceipt, "before" | "after") => Acknowledgement,
            (SemanticSettlement, "before" | "after") => Result,
            (SemanticContext, "before" | "after") => Text(256),
            (
                SemanticReceipt | SemanticStop | SemanticReport | SemanticSettlement,
                "execution_id",
            ) => Text(256),
            (_, "Reordered") => Reorder,
            (Reorder, "before") => QueueEntries,
            (Reorder, "after") => QueueIds,
            (_, "Admitted" | "Selected" | "Removed") => QueueEntry,
            (QueueEntry, "id") => Text(256),
            (Provider, "name" | "model_id") => Text(256),
            (Provider, "context") => Text(4096),
            (Actor, _) => Text(256),
            (_, "projection") => FinalizedComponents,
            (FinalizedComponent, "Operation") => Error,
            (FinalizedComponent, "PermissionDeliveryAndAudit") => Generic,
            (Metadata, "acknowledgement") => Acknowledgement,
            (Acknowledgement, "Failed") => FailedAcknowledgement,
            (FailedAcknowledgement, "audit") => Error,
            (FailedAcknowledgement, "storage") => StorageError,
            (StorageError, "Io" | "Corrupt") => StorageDiagnostic,
            (StorageError, "ShutdownFailures") => StorageChildren,
            (StorageChildren, "read" | "runtime") => StorageError,
            (Error, "Storage") | (ErrorBody(true), "error") => StorageError,
            (Metadata, "execution_id") | (Event, "execution_id" | "message_id") => Text(256),
            (Metadata, "user_message") => Text(ExecutionRequest::MAX_MESSAGE_BYTES),
            (Metadata, "user_images") => Images,
            (Metadata, "user_files") => Files,
            (FileLink, "path") => Text(LinkedFile::MAX_PATH_BYTES),
            (Image, "digest") => Text(DIGEST_BYTES),
            (Image, "media_type") => Text(MEDIA_TYPE_BYTES),
            (Event, "update") => Update,
            (Update, "Text" | "Thought") => Text(MAX_MESSAGE_CHUNK_BYTES),
            (Update, "Tool") | (Review, "tool") => Tool,
            (Update, "PermissionRequested" | "PermissionCancelled") => Review,
            (Update, "ReviewDeclined") => Decline,
            (Update, "QuestionAsked" | "QuestionClosed") => Ask,
            // Plural shapes are the arrays; `element` gives the singular for
            // each item, which is where the field bounds below apply. Mapping a
            // field straight to the singular skipped that step, so every saved
            // question decoded as generic and none of these bounds were read.
            (Ask, "questions") => AskedQuestions,
            (Asked, "options") => AskedOptions,
            (Ask, "id") => Text(256),
            (Ask, "message") | (Asked, "prompt" | "header") => Text(MAX_QUESTION_TEXT_BYTES),
            (Asked, "key" | "free_text_key") => Text(MAX_QUESTION_KEY_BYTES),
            (AskedOption, "value" | "label" | "description") => Text(MAX_QUESTION_TEXT_BYTES),
            (Tool, "content") => Content,
            (Tool, "locations") => Locations,
            (Tool, "mcp_tool") => McpTool,
            (McpTool, "server" | "tool") => Text(MAX_MCP_NAME_BYTES),
            (ContentItem, "Structured") => Text(MAX_STRUCTURED_RESULT_BYTES),
            (Tool, "id") | (Review, "id" | "execution_id" | "tool_id" | "session_id") => Text(256),
            (Review, "options") => Options,
            (Decline, "id") => Text(20),
            (Decline, "tool") => Text(128),
            (Decline, "reason" | "delivery") => Text(32),
            (Error, "BeforeInvocationHook") => Generic,
            (Error, "Provider") => ProviderError,
            (ProviderError, "diagnostic") => Text(ProviderDiagnostic::MAX_BYTES),
            (Error, "ImageInputMediaType") => Text(MEDIA_TYPE_BYTES),
            (
                Error,
                "Configuration" | "Unsupported" | "InvalidInput" | "Protocol" | "Transport",
            ) => Text(ERROR_BYTES),
            (Error, "StorageDuringClose" | "StorageInitialization" | "StorageAfterExecution") => {
                ErrorBody(true)
            }
            (Error, _) => ErrorBody(false),
            (Result, "Err") => Error,
            (ErrorBody(false), "error") => Error,
            (_, "actor" | "Client") => Actor,
            (_, "result" | "execution_result" | "cleanup_result" | "resources" | "audit") => Result,
            (
                _,
                "failure" | "operation_failure" | "completion_failure" | "first_error"
                | "subsequent_error" | "operation_error" | "cleanup_error" | "delivery_error",
            ) => Error,
            (_, "failures") => Hooks,
            _ => Generic,
        }
    }
    /// Whether an object of this shape may hold `key` at all. A saved image is
    /// exactly a digest, a media type, and a size, and a saved file link is
    /// exactly a path: anything else is refused before its value is read,
    /// rather than after the record is built.
    pub(super) fn allows(self, key: &str) -> bool {
        match self {
            Image => matches!(key, "digest" | "media_type" | "size"),
            FileLink => key == "path",
            McpTool => matches!(key, "server" | "tool"),
            ProviderError => matches!(key, "code" | "diagnostic"),
            FailedAcknowledgement => matches!(key, "audit" | "storage"),
            StorageError => matches!(
                key,
                "Io" | "Corrupt" | "ChangesRequired" | "Unresolved" | "ShutdownFailures"
            ),
            StorageChildren => matches!(key, "read" | "runtime"),
            _ => true,
        }
    }
    pub(super) fn element(self) -> Self {
        match self {
            Self::Invocations => Self::Invocation,
            Self::Events => Self::Event,
            Self::Images => Self::Image,
            Self::Files => Self::FileLink,
            Self::SemanticChanges => Self::Semantic,
            Self::FinalizedComponents => Self::FinalizedComponent,
            Self::Hooks => Self::Hook,
            Self::QueueEntries => Self::QueueEntry,
            Self::QueueIds => Self::Text(256),
            Self::AskedQuestions => Self::Asked,
            Self::AskedOptions => Self::AskedOption,
            Self::Content => Self::ContentItem,
            _ => Self::Generic,
        }
    }
    pub(super) fn array_limit(self) -> usize {
        match self {
            Self::Invocations => SessionSnapshot::MAX_INVOCATIONS,
            Self::Events => MAX_RETAINED_OUTPUT_EVENTS,
            Self::QueueHistory => {
                QueueHistoryRecord::maximum_entries(SessionSnapshot::MAX_INVOCATIONS)
            }
            Self::SemanticChanges => 262_144 + 16 * 1024,
            Self::Hooks => 128,
            Self::QueueEntries | Self::QueueIds => QueueOrderChange::MAX_PENDING,
            // The message's own constructor refuses more; refuse them here
            // before the excess references are built.
            Self::Images => UserMessage::MAX_IMAGES,
            // The same, for the paths a message points at.
            Self::Files => UserMessage::MAX_FILES,
            // Collection slots alone cannot exceed the live 32 MiB tool/review
            // budget, even when every element carries an empty payload.
            Self::Content => LARGE_STRING / size_of::<ToolContent>(),
            Self::Locations => LARGE_STRING / size_of::<FileLocation>(),
            Self::Options => LARGE_STRING / size_of::<PermissionOption>(),
            // Valid scheduling histories have at most three transitions.
            Self::Scheduling => 3,
            // Two categories retain at most 128 facts and one sticky marker each.
            Self::FinalizedComponents => 258,
            // The constructors refuse more; refuse them here, before the
            // excess questions and options are built.
            Self::AskedQuestions => MAX_QUESTIONS,
            Self::AskedOptions => MAX_QUESTION_OPTIONS,
            // Each collection element occupies retained storage. The per-change
            // structural budget below also applies to nested/empty elements.
            _ => 4 * 1024 * 1024,
        }
    }
}
