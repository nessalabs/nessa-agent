//! Select allocation budgets from storage field roles, without replacing schema validation.
use crate::application::agent_execution::executions::{
    limits::{MAX_MESSAGE_CHUNK_BYTES, MAX_RETAINED_OUTPUT_EVENTS},
    ExecutionRequest,
};
use crate::domain::agent_execution::{
    permissions::PermissionOption,
    prompts::UserMessage,
    tools::{FileLocation, ToolContent},
};
use std::mem::size_of;
use Shape::*;

pub(super) const LARGE_STRING: usize = 32 * 1024 * 1024;
pub(super) const ERROR_BYTES: usize = 1024 * 1024;
pub(super) const KEY_BYTES: usize = 128;
/// `sha256:` and 64 hexadecimal digits: the one text form of a digest.
const DIGEST_BYTES: usize = 71;
/// Longer than any image media type the domain names (`image/jpeg` is ten).
const MEDIA_TYPE_BYTES: usize = 16;

#[derive(Clone, Copy)]
pub(super) enum Shape {
    Index,
    Count,
    Record,
    Changes,
    Change,
    Metadata,
    Images,
    Image,
    Provider,
    Actor,
    Events,
    Event,
    Update,
    Scheduling,
    Reorders,
    Reorder,
    QueueEntries,
    QueueEntry,
    QueueIds,
    Tool,
    Review,
    Content,
    Locations,
    Options,
    Generic,
    Result,
    Error,
    ErrorBody(bool),
    Hooks,
    Hook,
    Text(usize),
}
impl Shape {
    pub(super) fn string_limit(self) -> usize {
        match self {
            Self::Text(limit) => limit,
            Self::Error => KEY_BYTES,
            _ => LARGE_STRING,
        }
    }
    pub(super) fn field(self, key: &str) -> Self {
        match (self, key) {
            (Record, "queue_history") => Reorders,
            (_, "Reordered") => Reorder,
            (Reorder, "before") => QueueEntries,
            (Reorder, "after") => QueueIds,
            (_, "Admitted" | "Selected" | "Removed") => QueueEntry,
            (QueueEntry, "id") => Text(256),
            (Record, "provider") => Provider,
            (Record, "invocation_count") => Count,
            (Change, "index") => Index,
            (Record, "invocations") => Changes,
            (Record, "id" | "provider_session_id") => Text(256),
            (Provider, "name" | "model_id") => Text(256),
            (Provider, "context") => Text(4096),
            (Actor, _) => Text(256),
            (Change, "metadata") => Metadata,
            (Change, "events") => Events,
            (Change, "scheduling") => Scheduling,
            (Metadata, "execution_id") | (Event, "execution_id" | "message_id") => Text(256),
            (Metadata, "user_message") => Text(ExecutionRequest::MAX_MESSAGE_BYTES),
            (Metadata, "user_images") => Images,
            (Image, "digest") => Text(DIGEST_BYTES),
            (Image, "media_type") => Text(MEDIA_TYPE_BYTES),
            (Event, "update") => Update,
            (Update, "Text" | "Thought") => Text(MAX_MESSAGE_CHUNK_BYTES),
            (Update, "Tool") | (Review, "tool") => Tool,
            (Update, "PermissionRequested" | "PermissionCancelled") => Review,
            (Tool, "content") => Content,
            (Tool, "locations") => Locations,
            (Tool, "id") | (Review, "id" | "execution_id" | "tool_id" | "session_id") => Text(256),
            (Review, "options") => Options,
            (Error, "BeforeInvocationHook") => Generic,
            (Error, "ImageInputMediaType") => Text(MEDIA_TYPE_BYTES),
            (
                Error,
                "Configuration" | "Unsupported" | "InvalidInput" | "Protocol" | "Transport",
            ) => Text(ERROR_BYTES),
            (
                Error,
                "Storage"
                | "StorageDuringClose"
                | "StorageInitialization"
                | "StorageAfterExecution",
            ) => ErrorBody(true),
            (Error, _) => ErrorBody(false),
            (Result, "Err") => Error,
            (ErrorBody(false), "error") => Error,
            (_, "actor" | "Client") => Actor,
            (_, "result" | "execution_result" | "cleanup_result" | "resources" | "audit") => Result,
            (
                _,
                "failure" | "operation_failure" | "first_error" | "subsequent_error"
                | "operation_error" | "cleanup_error" | "delivery_error",
            ) => Error,
            (_, "failures") => Hooks,
            _ => Generic,
        }
    }
    /// Whether an object of this shape may hold `key` at all. A saved image is
    /// exactly a digest, a media type, and a size: anything else is refused
    /// before its value is read, rather than after the record is built.
    pub(super) fn allows(self, key: &str) -> bool {
        !matches!(self, Image) || matches!(key, "digest" | "media_type" | "size")
    }
    pub(super) fn element(self) -> Self {
        match self {
            Self::Images => Self::Image,
            Self::Changes => Self::Change,
            Self::Events => Self::Event,
            Self::Hooks => Self::Hook,
            Self::Reorders => Self::Reorder,
            Self::QueueEntries => Self::QueueEntry,
            Self::QueueIds => Self::Text(256),
            _ => Self::Generic,
        }
    }
    pub(super) fn array_limit(self) -> usize {
        match self {
            Self::Changes => usize::MAX,
            Self::Events => MAX_RETAINED_OUTPUT_EVENTS,
            Self::Hooks => 128,
            Self::Reorders => usize::MAX,
            Self::QueueEntries | Self::QueueIds => 64,
            // The message's own constructor refuses more; refuse them here
            // before the excess references are built.
            Self::Images => UserMessage::MAX_IMAGES,
            // Collection slots alone cannot exceed the live 32 MiB tool/review
            // budget, even when every element carries an empty payload.
            Self::Content => LARGE_STRING / size_of::<ToolContent>(),
            Self::Locations => LARGE_STRING / size_of::<FileLocation>(),
            Self::Options => LARGE_STRING / size_of::<PermissionOption>(),
            // Valid scheduling histories have at most three transitions.
            Self::Scheduling => 3,
            // Each collection element occupies retained storage. The per-change
            // structural budget below also applies to nested/empty elements.
            _ => 4 * 1024 * 1024,
        }
    }
}
