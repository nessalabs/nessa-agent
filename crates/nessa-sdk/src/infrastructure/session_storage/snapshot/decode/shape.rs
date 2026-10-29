//! Select allocation budgets from storage field roles, without replacing schema validation.
use crate::application::agent_execution::agents::{DiagnosticTreeLimits, ProviderDiagnostic};
use crate::application::agent_execution::executions::{
    limits::{MAX_MESSAGE_CHUNK_BYTES, MAX_RETAINED_OUTPUT_EVENTS},
    ExecutionRequest,
};
use crate::application::agent_execution::sessions::{
    ArtifactName, ArtifactRecord, CurrentLease, HandoffId, HandoffRecord,
    LeaseRecord as SavedLeaseRecord, NoteId, NoteRecord, NoteText, QueueHistoryRecord,
    SessionSnapshot,
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, GrantInitiator, QueueOrderChange},
    leases::LeaseId,
    permissions::PermissionOption,
    prompts::{
        AppModelContext as DomainAppModelContext, CarriedHistory, CarriedTurn, LinkedFile,
        McpAppSource, UploadedFileReference, UserMessage,
    },
    questions::{
        MAX_KEY_BYTES as MAX_QUESTION_KEY_BYTES, MAX_OPTIONS as MAX_QUESTION_OPTIONS,
        MAX_QUESTIONS, MAX_TEXT_BYTES as MAX_QUESTION_TEXT_BYTES,
    },
    tools::{
        FileLocation, ToolContent, MAX_MCP_ARGUMENTS_BYTES, MAX_MCP_NAME_BYTES,
        MAX_STRUCTURED_RESULT_BYTES, MAX_TOOL_INPUT_BYTES,
    },
};
use crate::domain::common::value_objects::MediaType;
use crate::infrastructure::session_storage::save_group::{
    GroupCheckpoint as CheckpointMetadata, MetadataValueKind, SaveIdentity as IdentityMetadata,
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
    GroupCheckpoint,
    SaveIdentity,
    FixedBytes(usize),
    Number,
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
    SnapshotLease,
    LeaseRecords,
    LeaseRecord,
    Artifacts,
    Artifact,
    Notes,
    Note,
    NoteIds,
    NoteRetirement,
    NoteRetired,
    Handoffs,
    Handoff,
    HandoffKind,
    HandoffFork,
    HandoffSwitch,
    Carried,
    CarriedTurns,
    CarriedTurn,
    Metadata,
    Images,
    Image,
    Files,
    FileLink,
    Uploads,
    UploadedFile,
    App,
    AppModelContexts,
    AppModelContext,
    Provider,
    Actor,
    Initiation,
    Initiator,
    Grant,
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
    Deadline,
    DeadlineNotice,
    Decline,
    StoppingRule,
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
    AuthenticationError,
    ErrorBody(bool),
    Hooks,
    Hook,
    Text(usize),
}
impl Shape {
    fn metadata_field(kind: MetadataValueKind) -> Self {
        match kind {
            MetadataValueKind::Identity => Self::SaveIdentity,
            MetadataValueKind::FixedBytes(length) => Self::FixedBytes(length),
            MetadataValueKind::Number => Self::Number,
        }
    }
    pub(super) fn string_limit(self) -> usize {
        match self {
            Self::Text(limit) => limit,
            Self::StorageDiagnostic => ERROR_BYTES,
            Self::Acknowledgement
            | Self::StorageError
            | Self::Error
            | Self::Initiation
            | Self::Initiator
            | Self::Grant => KEY_BYTES,
            Self::GroupCheckpoint | Self::SaveIdentity | Self::FixedBytes(_) | Self::Number => 0,
            _ => LARGE_STRING,
        }
    }
    pub(super) fn field(self, key: &str) -> Self {
        match (self, key) {
            (Checkpoint, "snapshot") => Snapshot,
            (Checkpoint, "group") => GroupCheckpoint,
            (GroupCheckpoint, field) => {
                CheckpointMetadata::field_kind(field).map_or(Generic, Self::metadata_field)
            }
            (SaveIdentity, field) => {
                IdentityMetadata::field_kind(field).map_or(Generic, Self::metadata_field)
            }
            (
                Checkpoint,
                "receiver" | "origin" | "stream" | "incarnation" | "schema" | "access_epoch",
            ) => Text(256),
            (Snapshot, "id" | "context") => Text(256),
            (Snapshot, "provider") => Provider,
            (Snapshot, "invocations") => Invocations,
            (Snapshot, "queue_history") => QueueHistory,
            (Snapshot, "lease") => SnapshotLease,
            (SnapshotLease, "records") => LeaseRecords,
            (Semantic, "Lease") => LeaseRecord,
            (LeaseRecord, "kind") => Text(SavedLeaseRecord::MAX_UNREADABLE_KIND_BYTES),
            (LeaseRecord, "body") => Text(SavedLeaseRecord::MAX_UNREADABLE_BODY_BYTES),
            (Snapshot, "artifacts") => Artifacts,
            (Semantic, "Artifact") => Artifact,
            (Artifact, "lease") => Text(LeaseId::MAX_BYTES),
            (Artifact, "turn") => Text(ExecutionId::MAX_BYTES),
            (Artifact, "name") => Text(ArtifactName::MAX_BYTES),
            (Artifact, "digest") => Text(DIGEST_BYTES),
            (Artifact, "media_type") => Text(MediaType::MAX_BYTES),
            (Snapshot, "notes") => Notes,
            (Semantic, "Note") => Note,
            (Note, "id") => Text(NoteId::MAX_BYTES),
            (Note, "text") => Text(NoteText::MAX_BYTES),
            (Note, "delivery") => Text(KEY_BYTES),
            (Note, "author") => Initiator,
            (Note, "accepted_at_ms") => Number,
            (Note, "after_execution") => Text(ExecutionId::MAX_BYTES),
            (Note, "retired") | (NoteRetired, "retirement") => NoteRetirement,
            (NoteRetirement, "reason") => Text(KEY_BYTES),
            (NoteRetirement, "at_ms") => Number,
            (Semantic, "NoteRetired") => NoteRetired,
            (NoteRetired, "id") => Text(NoteId::MAX_BYTES),
            (Metadata, "notes") => NoteIds,
            (Metadata, "handoff") => Text(HandoffId::MAX_BYTES),
            (Snapshot, "handoffs") => Handoffs,
            (Semantic, "Handoff") => Handoff,
            (Handoff, "id") => Text(HandoffId::MAX_BYTES),
            (Handoff, "kind") => HandoffKind,
            (Handoff, "at_ms") => Number,
            (Handoff, "after_execution") => Text(ExecutionId::MAX_BYTES),
            (HandoffKind, "Switch") => HandoffSwitch,
            (HandoffSwitch, "before" | "after") => Provider,
            (HandoffKind, "Fork") => HandoffFork,
            (HandoffFork, "source") => Text(256),
            (HandoffFork, "history") => Carried,
            (Carried, "turns") => CarriedTurns,
            (CarriedTurn, "said" | "reply") => Text(CarriedTurn::MAX_PART_BYTES),
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
            (Metadata, "resumed_from") => Text(ExecutionId::MAX_BYTES),
            (Metadata, "initiation") => Initiation,
            (Initiation, "initiator") => Initiator,
            (Initiation, "tools") => Text(KEY_BYTES),
            (Initiator, "Grant") => Grant,
            (Grant, "grant") => Text(GrantInitiator::MAX_GRANT_BYTES),
            (Grant, "policy_revision") => Number,
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
            (Metadata, "user_uploads") => Uploads,
            (UploadedFile, "name") => Text(UploadedFileReference::MAX_NAME_BYTES),
            (UploadedFile, "digest") => Text(DIGEST_BYTES),
            (UploadedFile, "media_type") => Text(127),
            (Metadata, "user_app") | (AppModelContext, "app") => App,
            (Metadata, "user_app_model_context") => AppModelContexts,
            (App, "execution_id") => Text(ExecutionId::MAX_BYTES),
            (App, "tool_id") => Text(McpAppSource::MAX_TOOL_ID_BYTES),
            (App, "server" | "tool") => Text(MAX_MCP_NAME_BYTES),
            (AppModelContext, "text" | "structured_content") => {
                Text(DomainAppModelContext::MAX_BYTES)
            }
            (AppModelContext, "update") => Text(DomainAppModelContext::MAX_UPDATE_BYTES),
            (FileLink, "path") => Text(LinkedFile::MAX_PATH_BYTES),
            (Image, "digest") => Text(DIGEST_BYTES),
            (Image, "media_type") => Text(MEDIA_TYPE_BYTES),
            (Event, "update") => Update,
            (Event, "at_ms") => Number,
            (Update, "Text" | "Thought") => Text(MAX_MESSAGE_CHUNK_BYTES),
            (Update, "Tool") | (Review, "tool") => Tool,
            (Update, "PermissionRequested" | "PermissionCancelled") => Review,
            (Update, "DeadlineAnswered") => DeadlineNotice,
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
            (Tool, "mcp_arguments") => Text(MAX_MCP_ARGUMENTS_BYTES),
            (Tool, "input") => Text(MAX_TOOL_INPUT_BYTES),
            (McpTool, "server" | "tool") => Text(MAX_MCP_NAME_BYTES),
            (ContentItem, "Structured") => Text(MAX_STRUCTURED_RESULT_BYTES),
            (Tool, "id") | (Review, "id" | "execution_id" | "tool_id" | "session_id") => Text(256),
            (Review, "options") => Options,
            (Review, "deadline") => Deadline,
            (Deadline, "class" | "effect") => Text(32),
            (Deadline, "reason") => {
                Text(crate::domain::agent_execution::permissions::MAX_DEADLINE_REASON_BYTES)
            }
            (Deadline, "wait_ms") => Number,
            (DeadlineNotice, "id") => Text(256),
            (DeadlineNotice, "effect") => Text(16),
            (DeadlineNotice, "reason") => {
                Text(crate::domain::agent_execution::permissions::MAX_DEADLINE_REASON_BYTES)
            }
            (Decline, "id") => Text(20),
            (Decline, "tool") => Text(128),
            (Decline, "reason" | "delivery" | "outcome") => Text(32),
            (Decline, "rule") => StoppingRule,
            (StoppingRule, "rule_id" | "revision") => Text(
                crate::application::agent_execution::permissions::ActionContext::MAX_IDENTITY_BYTES,
            ),
            (StoppingRule, "granted_by") => Actor,
            (Error, "BeforeInvocationHook") => Generic,
            (Error, "Provider") => ProviderError,
            (Error, "AuthenticationRequired") => AuthenticationError,
            (ProviderError | AuthenticationError, "diagnostic") => {
                Text(ProviderDiagnostic::MAX_BYTES)
            }
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
            GroupCheckpoint => CheckpointMetadata::field_kind(key).is_some(),
            SaveIdentity => IdentityMetadata::field_kind(key).is_some(),
            FixedBytes(_) | Number => false,
            Image => matches!(key, "digest" | "media_type" | "size"),
            FileLink => key == "path",
            UploadedFile => matches!(key, "digest" | "media_type" | "size" | "name"),
            App => matches!(key, "execution_id" | "tool_id" | "server" | "tool"),
            AppModelContext => matches!(key, "app" | "update" | "text" | "structured_content"),
            McpTool => matches!(key, "server" | "tool"),
            SnapshotLease => matches!(key, "revision" | "records"),
            LeaseRecord => matches!(key, "kind" | "body"),
            Artifact => matches!(
                key,
                "lease" | "turn" | "name" | "digest" | "media_type" | "size" | "actor"
            ),
            Note => matches!(
                key,
                "id" | "text"
                    | "delivery"
                    | "actor"
                    | "author"
                    | "accepted_at_ms"
                    | "after_execution"
                    | "retired"
            ),
            NoteRetirement => matches!(key, "reason" | "at_ms"),
            NoteRetired => matches!(key, "id" | "retirement"),
            Handoff => matches!(key, "id" | "kind" | "actor" | "at_ms" | "after_execution"),
            HandoffKind => matches!(key, "Switch" | "Fork"),
            HandoffFork => matches!(key, "source" | "history"),
            Carried => matches!(key, "turns" | "omitted"),
            HandoffSwitch => matches!(key, "before" | "after"),
            CarriedTurn => matches!(key, "said" | "reply"),
            ProviderError => matches!(key, "code" | "diagnostic"),
            AuthenticationError => key == "diagnostic",
            FailedAcknowledgement => matches!(key, "audit" | "storage"),
            Initiation => matches!(key, "initiator" | "tools"),
            Initiator => key == "Grant",
            Grant => matches!(key, "grant" | "policy_revision"),
            StorageError => matches!(
                key,
                "Io" | "Corrupt" | "ChangesRequired" | "Unresolved" | "ShutdownFailures"
            ),
            StorageChildren => matches!(key, "read" | "runtime"),
            Decline => matches!(
                key,
                "id" | "tool" | "reason" | "outcome" | "delivery" | "rule"
            ),
            StoppingRule => matches!(key, "rule_id" | "revision" | "granted_by"),
            _ => true,
        }
    }
    pub(super) fn element(self) -> Self {
        match self {
            Self::FixedBytes(_) => Self::Number,
            Self::Invocations => Self::Invocation,
            Self::Events => Self::Event,
            Self::Images => Self::Image,
            Self::Files => Self::FileLink,
            Self::Uploads => Self::UploadedFile,
            Self::AppModelContexts => Self::AppModelContext,
            Self::SemanticChanges => Self::Semantic,
            Self::LeaseRecords => Self::LeaseRecord,
            Self::Artifacts => Self::Artifact,
            Self::Notes => Self::Note,
            Self::Handoffs => Self::Handoff,
            Self::CarriedTurns => Self::CarriedTurn,
            Self::NoteIds => Self::Text(NoteId::MAX_BYTES),
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
            Self::FixedBytes(length) => length,
            Self::Number | Self::GroupCheckpoint | Self::SaveIdentity => 0,
            Self::Invocations => SessionSnapshot::MAX_INVOCATIONS,
            Self::Events => MAX_RETAINED_OUTPUT_EVENTS,
            Self::QueueHistory => {
                QueueHistoryRecord::maximum_entries(SessionSnapshot::MAX_INVOCATIONS)
            }
            Self::SemanticChanges => 262_144 + 16 * 1024,
            Self::Hooks => 128,
            Self::LeaseRecords => CurrentLease::MAX_RECORDS,
            Self::Artifacts => ArtifactRecord::MAX_PER_CONVERSATION,
            Self::Notes => NoteRecord::MAX_PER_CONVERSATION,
            Self::Handoffs => HandoffRecord::MAX_PER_CONVERSATION,
            Self::CarriedTurns => CarriedHistory::MAX_TURNS,
            // A turn takes no more notes than a message carries.
            Self::NoteIds => UserMessage::MAX_NOTES,
            Self::QueueEntries | Self::QueueIds => QueueOrderChange::MAX_PENDING,
            // The message's own constructor refuses more; refuse them here
            // before the excess references are built.
            Self::Images => UserMessage::MAX_IMAGES,
            // The same, for the paths a message points at.
            Self::Files | Self::Uploads => UserMessage::MAX_FILES,
            // And the apps' contexts it carries.
            Self::AppModelContexts => UserMessage::MAX_APP_MODEL_CONTEXTS,
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
