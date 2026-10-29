use super::{
    cancellation::Cancellation as InvocationCancellation,
    errors::{decode_result, Outcome, SavedError, StorageFailure},
    initiation::Initiation,
    permissions::{self, Actor, Cancellation, Choice, Input},
    settlement::Settlement,
    tools::{corrupt, Tool},
};
use crate::application::agent_execution::{
    executions::{
        limits::validate_observation_id, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
        SubmissionMode,
    },
    permissions::{ApprovalRuleReference, DeclinedReview},
    providers::{ExecutionReport, ProviderIdentity},
    sessions::{
        storage::{InvocationRecord, StorageError, SubmissionAcknowledgement},
        HandoffId, NoteId,
    },
};
use crate::domain::{
    agent_execution::{
        executions::{ExecutionId, MessageChunk, MessageId, MessageKind},
        permissions::{
            PermissionEffect, PermissionId, PermissionReviewDeadline, PermissionToolClass,
            RefusalOutcome, ReviewDecline, ReviewDeclineId, ReviewDeclineReason,
            ReviewDeclineStage,
        },
        prompts::{
            AppModelContext as DomainAppModelContext, ImageReference, LinkedFile, McpAppSource,
            MessageSender, PromptText, UploadedFileReference, UserMessage,
        },
        questions::{AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId},
        sessions::ExecutionSessionId,
        tools::{McpTool, ToolCallId},
    },
    common::value_objects::{ImageMediaType, MediaType, Sha256Digest},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provider {
    pub(super) name: String,
    pub(super) model_id: String,
    pub(super) context: String,
}
/// One image a saved user message refers to; the bytes are never saved here.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Image {
    pub(super) digest: String,
    pub(super) media_type: String,
    pub(super) size: u64,
}
impl From<&ImageReference> for Image {
    fn from(value: &ImageReference) -> Self {
        Self {
            digest: value.digest().to_string(),
            media_type: value.media_type().as_str().into(),
            size: value.size(),
        }
    }
}
impl Image {
    fn decode(self) -> Result<ImageReference, StorageError> {
        ImageReference::new(
            Sha256Digest::parse(&self.digest).map_err(corrupt)?,
            ImageMediaType::parse(&self.media_type).map_err(corrupt)?,
            self.size,
        )
        .map_err(corrupt)
    }
}
/// A durable uploaded input, independent of its eventual placement path.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UploadedFile {
    digest: String,
    media_type: String,
    size: u64,
    name: String,
}
impl From<&UploadedFileReference> for UploadedFile {
    fn from(file: &UploadedFileReference) -> Self {
        Self {
            digest: file.digest().to_string(),
            media_type: file.media_type().as_str().into(),
            size: file.size(),
            name: file.name().into(),
        }
    }
}
impl UploadedFile {
    fn decode(self) -> Result<UploadedFileReference, StorageError> {
        UploadedFileReference::new(
            Sha256Digest::parse(&self.digest).map_err(corrupt)?,
            MediaType::parse(&self.media_type).map_err(corrupt)?,
            self.size,
            self.name,
        )
        .map_err(corrupt)
    }
}
/// One file a saved user message points at. The path is all there is to save:
/// nothing was opened when the message was written and nothing is opened when
/// it is read back, so there is no content to have gone stale.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileLink {
    pub(super) path: String,
}
impl From<&LinkedFile> for FileLink {
    fn from(value: &LinkedFile) -> Self {
        Self {
            path: value.path().into(),
        }
    }
}
impl FileLink {
    /// Rebuild the value object, so a path edited on disk into one the domain
    /// would never have accepted is a corrupt record rather than a link the
    /// agent is handed.
    fn decode(self) -> Result<LinkedFile, StorageError> {
        LinkedFile::new(self.path).map_err(corrupt)
    }
}
/// The MCP App a saved user message was written by, or carries the context
/// of: its tool call, and the server and tool that call was to.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct App {
    pub(super) execution_id: String,
    pub(super) tool_id: String,
    pub(super) server: String,
    pub(super) tool: String,
}
impl From<&McpAppSource> for App {
    fn from(value: &McpAppSource) -> Self {
        Self {
            execution_id: value.execution_id().as_str().into(),
            tool_id: value.tool_id().as_str().into(),
            server: value.tool().server().into(),
            tool: value.tool().tool().into(),
        }
    }
}
impl App {
    /// Rebuild the value object, so an identity or name edited on disk into
    /// one the domain refuses is a corrupt record.
    fn decode(self) -> Result<McpAppSource, StorageError> {
        McpAppSource::new(
            ExecutionId::new(self.execution_id).map_err(corrupt)?,
            ToolCallId::new(self.tool_id).map_err(corrupt)?,
            McpTool::new(self.server, self.tool).map_err(corrupt)?,
        )
        .map_err(corrupt)
    }
}
/// What one app gave the model to know, saved with the message it went
/// with, so a restored message sends what was taken for it
/// (`semantic_admission_restores_who_wrote_it_and_what_apps_gave_with_it`).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppModelContext {
    pub(super) app: App,
    pub(super) update: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub(super) text: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub(super) structured_content: Option<String>,
}
impl From<&DomainAppModelContext> for AppModelContext {
    fn from(value: &DomainAppModelContext) -> Self {
        Self {
            app: value.app().into(),
            update: value.update_id().into(),
            text: value.text().map(str::to_owned),
            structured_content: value.structured_content().map(str::to_owned),
        }
    }
}
impl AppModelContext {
    /// Rebuild the value object. An empty text is refused rather than read
    /// as none: no context is saved with one, so a record holding one was
    /// changed, and is not repaired.
    fn decode(self) -> Result<DomainAppModelContext, StorageError> {
        if self.text.as_deref() == Some("") {
            return Err(corrupt("an app context saved with an empty text"));
        }
        DomainAppModelContext::new(
            self.app.decode()?,
            &self.update,
            self.text,
            self.structured_content,
        )
        .map_err(corrupt)?
        .ok_or_else(|| corrupt("an app context saved with neither part"))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Metadata {
    /// Absence is no resumption evidence; never infer origin from input shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) resumed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) decline: Option<SavedDecline>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cancellation_delivery: Option<SavedCancellation>,
    pub(super) target_event_offset: Option<usize>,
    pub(super) submission: Submission,
    pub(super) execution_id: String,
    /// Empty for a message of images or files alone.
    pub(super) user_message: String,
    pub(super) user_images: Vec<Image>,
    /// Empty when a message has no linked files.
    pub(super) user_files: Vec<FileLink>,
    /// Absent in records written before stored-file input existed means none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) user_uploads: Vec<UploadedFile>,
    /// The app that wrote the message on the person's behalf; `null` when
    /// the person did. Required: a record without it is not of this shape
    /// (`deserialize_with` keeps serde from reading its absence as `None`).
    #[serde(deserialize_with = "Option::deserialize")]
    pub(super) user_app: Option<App>,
    /// Application-generated continuation, never simultaneously an app message.
    pub(super) user_internal: bool,
    /// What apps gave the model with this message; empty when none did.
    pub(super) user_app_model_context: Vec<AppModelContext>,
    pub(super) estimated_input_tokens: u64,
    pub(super) reserved_output_tokens: u32,
    pub(super) actor: Actor,
    /// Who started the turn and whether its tools were checked.
    pub(super) initiation: Initiation,
    /// The notes the turn took, in the order it took them; absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) notes: Vec<String>,
    /// The handoff the turn took; absent when it took none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) handoff: Option<String>,
    pub(super) acknowledgement: Acknowledgement,
    pub(super) provider_report: Option<Settlement>,
    pub(super) local_cancellation: Option<InvocationCancellation>,
    pub(super) local_outcome: Option<Outcome>,
    pub(super) cancellation: Option<InvocationCancellation>,
    pub(super) result: Option<Result<Outcome, SavedError>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Acknowledgement {
    Pending,
    Acknowledged,
    Failed {
        audit: Option<SavedError>,
        storage: Option<StorageFailure>,
    },
}
impl From<&SubmissionAcknowledgement> for Acknowledgement {
    fn from(value: &SubmissionAcknowledgement) -> Self {
        match value {
            SubmissionAcknowledgement::Pending => Self::Pending,
            SubmissionAcknowledgement::Acknowledged => Self::Acknowledged,
            SubmissionAcknowledgement::Failed { audit, storage } => Self::Failed {
                audit: audit.clone().map(Into::into),
                storage: storage.clone().map(Into::into),
            },
        }
    }
}
impl TryFrom<Acknowledgement> for SubmissionAcknowledgement {
    type Error = StorageError;

    fn try_from(value: Acknowledgement) -> Result<Self, Self::Error> {
        Ok(match value {
            Acknowledgement::Pending => Self::Pending,
            Acknowledgement::Acknowledged => Self::Acknowledged,
            Acknowledgement::Failed { audit, storage } => Self::Failed {
                audit: audit.map(TryInto::try_into).transpose()?,
                storage: storage.map(TryInto::try_into).transpose()?,
            },
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Event<T = String> {
    pub(super) message_id: Option<String>,
    pub(super) execution_id: String,
    pub(super) update: Update<T>,
    /// The hooks choice of the process start that produced the event. Absent
    /// for a start without one, and on events saved before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) native_hooks: Option<bool>,
    /// When the session received the event, in Unix milliseconds. Absent on
    /// events saved before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) at_ms: Option<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Update<T = String> {
    Finished(Outcome),
    Text(T),
    Thought(T),
    Tool(Tool),
    PermissionCancelled(Cancellation),
    ReviewDeclined {
        id: String,
        tool: Option<String>,
        reason: DeclineReason,
        outcome: DeclineOutcome,
        delivery: DeclineDelivery,
        /// Present exactly when `outcome` stops the turn or closes the session.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<StoppingRule>,
    },
    PermissionRequested {
        id: String,
        tool: Tool,
        input: Input,
        options: Vec<Choice>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deadline: Option<SavedDeadline>,
    },
    DeadlineAnswered {
        id: String,
        effect: SavedEffect,
        reason: String,
    },
    QuestionAsked {
        id: String,
        message: String,
        questions: Vec<Asked>,
    },
    QuestionClosed {
        id: String,
    },
}
/// One saved question: what was asked, and what could be chosen.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Asked {
    pub(super) key: String,
    pub(super) prompt: String,
    pub(super) header: Option<String>,
    pub(super) many: bool,
    /// The field the asker named for prose. A restored ask that lost it would
    /// send the answerer's words to a field the schema does not have.
    pub(super) free_text_key: Option<String>,
    /// Whether the asker said this may not be skipped. Lost on restore, a
    /// required question would quietly become an optional one.
    pub(super) required: bool,
    pub(super) options: Vec<AskedOption>,
}
/// One saved answer a question offered.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AskedOption {
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: Option<String>,
}
/// A deadline saved with the review that was waiting on it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedDeadline {
    pub(super) class: SavedClass,
    pub(super) wait_ms: u64,
    pub(super) effect: SavedEffect,
    pub(super) reason: String,
    /// Whether the selected refusal stops the turn.
    pub(super) stops_turn: bool,
    /// Unix milliseconds the live worker promised, when it promised one.
    /// Restoring it does not re-arm the review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) due_at_ms: Option<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SavedClass {
    ReadOnly,
    Other,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SavedEffect {
    Allow,
    Deny,
}
impl From<PermissionEffect> for SavedEffect {
    fn from(effect: PermissionEffect) -> Self {
        match effect {
            PermissionEffect::Allow => Self::Allow,
            PermissionEffect::Deny => Self::Deny,
        }
    }
}
impl From<SavedEffect> for PermissionEffect {
    fn from(effect: SavedEffect) -> Self {
        match effect {
            SavedEffect::Allow => Self::Allow,
            SavedEffect::Deny => Self::Deny,
        }
    }
}
/// The configured rule that required a stop, saved with the declined review.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoppingRule {
    pub(super) rule_id: String,
    pub(super) revision: String,
    pub(super) granted_by: Actor,
}

#[derive(Serialize, Deserialize)]
pub(super) enum DeclineReason {
    ToolNotReviewable,
    UnreadableRequest,
    UnusableOptions,
}
#[derive(Serialize, Deserialize)]
pub(super) enum DeclineOutcome {
    Continue,
    EndTurn,
    CloseSession,
}
#[derive(Serialize, Deserialize)]
pub(super) enum DeclineDelivery {
    Selected,
    WriteConfirmed,
    WriteUnconfirmed,
    WriteNotAttempted,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedDecline {
    event: super::scheduling::SchedulingEvent,
    acknowledged: bool,
}
impl From<&crate::application::agent_execution::sessions::DeclineDecision> for SavedDecline {
    fn from(value: &crate::application::agent_execution::sessions::DeclineDecision) -> Self {
        Self {
            event: value.event().clone().into(),
            acknowledged: value.acknowledged(),
        }
    }
}
impl SavedDecline {
    pub(super) fn decode(
        self,
    ) -> Result<crate::application::agent_execution::sessions::DeclineDecision, StorageError> {
        Ok(
            crate::application::agent_execution::sessions::DeclineDecision::new(
                self.event.decode()?,
                self.acknowledged,
            ),
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedCancellation {
    event: super::scheduling::SchedulingEvent,
    acknowledged: bool,
}
impl From<&crate::application::agent_execution::sessions::CancellationDecision>
    for SavedCancellation
{
    fn from(value: &crate::application::agent_execution::sessions::CancellationDecision) -> Self {
        Self {
            event: value.event().clone().into(),
            acknowledged: value.acknowledged(),
        }
    }
}
impl SavedCancellation {
    pub(super) fn decode(
        self,
    ) -> Result<crate::application::agent_execution::sessions::CancellationDecision, StorageError>
    {
        Ok(
            crate::application::agent_execution::sessions::CancellationDecision::new(
                self.event.decode()?,
                self.acknowledged,
            ),
        )
    }
}
impl From<&InvocationRecord> for Metadata {
    fn from(value: &InvocationRecord) -> Self {
        Self {
            resumed_from: value.resumed_from.as_ref().map(|id| id.as_str().into()),
            decline: value.decline.as_ref().map(Into::into),
            cancellation_delivery: value.cancellation_delivery.as_ref().map(Into::into),
            target_event_offset: value.target_event_offset,
            submission: value.submission.into(),
            execution_id: value.request.execution_id.as_str().into(),
            user_message: value.request.user_message.text_str().into(),
            user_images: value
                .request
                .user_message
                .images()
                .iter()
                .map(Into::into)
                .collect(),
            user_uploads: value
                .request
                .user_message
                .uploads()
                .iter()
                .map(UploadedFile::from)
                .collect(),
            user_files: value
                .request
                .user_message
                .files()
                .iter()
                .map(Into::into)
                .collect(),
            user_app: match value.request.user_message.sender() {
                MessageSender::Person | MessageSender::Internal => None,
                MessageSender::App(app) => Some(app.into()),
            },
            user_internal: matches!(value.request.user_message.sender(), MessageSender::Internal),
            user_app_model_context: value
                .request
                .user_message
                .app_model_context()
                .iter()
                .map(Into::into)
                .collect(),
            estimated_input_tokens: value.request.estimated_input_tokens,
            reserved_output_tokens: value.request.reserved_output_tokens,
            actor: (&value.actor).into(),
            initiation: (&value.initiation).into(),
            notes: value.notes.iter().map(|id| id.as_str().into()).collect(),
            handoff: value.handoff.as_ref().map(|id| id.as_str().into()),
            acknowledgement: (&value.acknowledgement).into(),
            provider_report: value.provider_report.clone().map(Into::into),
            local_cancellation: value.local_cancellation.as_ref().map(Into::into),
            local_outcome: value.local_outcome.map(Into::into),
            cancellation: value.cancellation.as_ref().map(Into::into),
            result: value
                .result
                .clone()
                .map(|result| result.map(Into::into).map_err(Into::into)),
        }
    }
}
fn saved_effect(effect: PermissionEffect) -> SavedEffect {
    effect.into()
}

pub(super) fn saved_deadline(deadline: &PermissionReviewDeadline) -> SavedDeadline {
    SavedDeadline {
        class: match deadline.class() {
            PermissionToolClass::ReadOnly => SavedClass::ReadOnly,
            PermissionToolClass::Other => SavedClass::Other,
        },
        wait_ms: crate::domain::agent_execution::permissions::wait_ms(deadline.wait()),
        effect: saved_effect(deadline.effect()),
        reason: deadline.reason().to_owned(),
        stops_turn: deadline.stops_turn(),
        due_at_ms: deadline.due_at_ms(),
    }
}

/// A deadline that is not one this build can record is absent. The review
/// then waits for a person, and the card does not promise a decision. A
/// wrong-typed deadline never reaches here: serde rejects it as corrupt
/// before this runs. Restoring does not arm the deadline: the provider
/// request died with the process that saved it.
pub(super) fn restore_deadline(saved: SavedDeadline) -> Option<PermissionReviewDeadline> {
    let class = match saved.class {
        SavedClass::ReadOnly => PermissionToolClass::ReadOnly,
        SavedClass::Other => PermissionToolClass::Other,
    };
    PermissionReviewDeadline::new(
        class,
        std::time::Duration::from_millis(saved.wait_ms),
        saved.effect.into(),
        saved.reason,
    )
    .ok()
    .and_then(|deadline| deadline.restored(saved.stops_turn, saved.due_at_ms).ok())
}

impl<'a> From<&'a ExecutionEvent> for Event<&'a str> {
    fn from(event: &'a ExecutionEvent) -> Self {
        Self {
            message_id: match event.update() {
                ExecutionUpdate::Message(chunk) => chunk.message_id().map(str::to_owned),
                _ => None,
            },
            execution_id: event.execution_id().as_str().into(),
            native_hooks: event.native_hooks(),
            at_ms: event.observed_at_ms(),
            update: match event.update() {
                ExecutionUpdate::Finished(outcome) => Update::Finished((*outcome).into()),
                ExecutionUpdate::Message(chunk) => match chunk.kind() {
                    MessageKind::Text => Update::Text(chunk.as_str()),
                    MessageKind::Thought => Update::Thought(chunk.as_str()),
                },
                ExecutionUpdate::Tool(tool) => Update::Tool(tool.into()),
                ExecutionUpdate::PermissionCancelled(cancellation) => {
                    Update::PermissionCancelled(cancellation.into())
                }
                ExecutionUpdate::ReviewDeclined(review) => Update::ReviewDeclined {
                    id: review.id().as_str().into(),
                    tool: review.decline().declared().map(str::to_owned),
                    reason: match review.decline().reason() {
                        ReviewDeclineReason::ToolNotReviewable => DeclineReason::ToolNotReviewable,
                        ReviewDeclineReason::UnreadableRequest => DeclineReason::UnreadableRequest,
                        ReviewDeclineReason::UnusableOptions => DeclineReason::UnusableOptions,
                    },
                    outcome: match review.decline().outcome() {
                        RefusalOutcome::Continue => DeclineOutcome::Continue,
                        RefusalOutcome::EndTurn => DeclineOutcome::EndTurn,
                        RefusalOutcome::CloseSession => DeclineOutcome::CloseSession,
                    },
                    delivery: match review.stage() {
                        ReviewDeclineStage::Selected => DeclineDelivery::Selected,
                        ReviewDeclineStage::WriteConfirmed => DeclineDelivery::WriteConfirmed,
                        ReviewDeclineStage::WriteUnconfirmed => DeclineDelivery::WriteUnconfirmed,
                        ReviewDeclineStage::WriteNotAttempted => DeclineDelivery::WriteNotAttempted,
                    },
                    rule: review.rule().map(|rule| StoppingRule {
                        rule_id: rule.rule_id().into(),
                        revision: rule.revision().into(),
                        granted_by: rule.granted_by().into(),
                    }),
                },
                ExecutionUpdate::PermissionRequested {
                    id,
                    tool_id,
                    observation,
                    input,
                    options,
                    deadline,
                } => Update::PermissionRequested {
                    id: id.as_str().into(),
                    tool: Tool::observation(tool_id, observation),
                    input: input.into(),
                    options: permissions::choices(options),
                    deadline: deadline.as_ref().map(saved_deadline),
                },
                ExecutionUpdate::DeadlineAnswered { id, effect, reason } => {
                    Update::DeadlineAnswered {
                        id: id.as_str().into(),
                        effect: saved_effect(*effect),
                        reason: reason.as_str().into(),
                    }
                }
                ExecutionUpdate::QuestionClosed { id } => Update::QuestionClosed {
                    id: id.as_str().into(),
                },
                ExecutionUpdate::QuestionAsked { id, question } => Update::QuestionAsked {
                    id: id.as_str().into(),
                    message: question.message().into(),
                    questions: question
                        .questions()
                        .iter()
                        .map(|asked| Asked {
                            key: asked.key().into(),
                            prompt: asked.prompt().into(),
                            header: asked.header().map(str::to_owned),
                            many: asked.shape() == AnswerShape::Many,
                            free_text_key: asked.free_text_key().map(str::to_owned),
                            required: asked.required(),
                            options: asked
                                .options()
                                .iter()
                                .map(|option| AskedOption {
                                    value: option.value().into(),
                                    label: option.label().into(),
                                    description: option.description().map(str::to_owned),
                                })
                                .collect(),
                        })
                        .collect(),
                },
            },
        }
    }
}
impl From<&ProviderIdentity> for Provider {
    fn from(value: &ProviderIdentity) -> Self {
        Self {
            name: value.name().into(),
            model_id: value.model_id().into(),
            context: value.context().into(),
        }
    }
}
impl Provider {
    pub(super) fn decode(self) -> Result<ProviderIdentity, StorageError> {
        ProviderIdentity::new(self.name, self.model_id, self.context).map_err(corrupt)
    }
}
impl Metadata {
    pub(super) fn decode(self) -> Result<InvocationRecord, StorageError> {
        Ok(InvocationRecord {
            decline: self.decline.map(SavedDecline::decode).transpose()?,
            cancellation_delivery: self
                .cancellation_delivery
                .map(SavedCancellation::decode)
                .transpose()?,
            resumed_from: self
                .resumed_from
                .map(ExecutionId::new)
                .transpose()
                .map_err(corrupt)?,
            target_event_offset: self.target_event_offset,
            submission: self.submission.into(),
            request: ExecutionRequest {
                execution_id: ExecutionId::new(self.execution_id).map_err(corrupt)?,
                // Saved text is empty exactly when the message was images or
                // files alone; the message's own constructor refuses one with
                // none of the three.
                user_message: UserMessage::new(
                    (!self.user_message.is_empty())
                        .then(|| PromptText::new(self.user_message))
                        .transpose()
                        .map_err(corrupt)?,
                    self.user_images
                        .into_iter()
                        .map(Image::decode)
                        .collect::<Result<_, _>>()?,
                    self.user_files
                        .into_iter()
                        .map(FileLink::decode)
                        .collect::<Result<_, _>>()?,
                    self.user_uploads
                        .into_iter()
                        .map(UploadedFile::decode)
                        .collect::<Result<_, _>>()?,
                )
                .map_err(corrupt)?
                .sent_by(match (self.user_internal, self.user_app) {
                    (false, None) => MessageSender::Person,
                    (true, None) => MessageSender::Internal,
                    (false, Some(app)) => MessageSender::App(app.decode()?),
                    (true, Some(_)) => return Err(corrupt("input has two senders")),
                })
                .with_app_model_context(
                    self.user_app_model_context
                        .into_iter()
                        .map(AppModelContext::decode)
                        .collect::<Result<_, _>>()?,
                )
                .map_err(corrupt)?,
                estimated_input_tokens: self.estimated_input_tokens,
                reserved_output_tokens: self.reserved_output_tokens,
            },
            actor: self.actor.decode()?,
            initiation: self.initiation.decode()?,
            notes: self
                .notes
                .into_iter()
                .map(|id| NoteId::new(id).map_err(corrupt))
                .collect::<Result<_, _>>()?,
            handoff: self.handoff.map(HandoffId::new).transpose()?,
            acknowledgement: self.acknowledgement.try_into()?,
            events: Vec::new(),
            scheduling: Vec::new(),
            provider_report: self
                .provider_report
                .map(ExecutionReport::try_from)
                .transpose()?,
            local_cancellation: self
                .local_cancellation
                .map(InvocationCancellation::decode)
                .transpose()?,
            local_outcome: self.local_outcome.map(Into::into),
            cancellation: self
                .cancellation
                .map(InvocationCancellation::decode)
                .transpose()?,
            result: self.result.map(decode_result).transpose()?,
        })
    }
}
/// The provider context a stored cancellation is read against. A cancellation
/// names the context in force when it was recorded.
pub(super) enum CancellationContext<'a> {
    /// A semantic fact, read with the context in force at that fact: the
    /// cancellation must name it.
    AtFact(&'a ExecutionSessionId),
    /// A checkpoint, which keeps only the final context. A cancellation from
    /// an earlier context is accepted: each was checked against its own
    /// context when its fact was recorded, and a checkpoint is written only
    /// from that validated state.
    Checkpoint,
}

impl Event {
    pub(super) fn decode(
        self,
        provider_context: CancellationContext<'_>,
        execution_id: &ExecutionId,
    ) -> Result<ExecutionEvent, StorageError> {
        if self.execution_id != execution_id.as_str() {
            return Err(corrupt("event belongs to a different invocation"));
        }
        let update = match self.update {
            Update::Finished(outcome) => ExecutionUpdate::Finished(outcome.into()),
            Update::Text(text) => ExecutionUpdate::Message(MessageChunk::text(text)),
            Update::Thought(text) => ExecutionUpdate::Message(MessageChunk::thought(text)),
            Update::Tool(tool) => ExecutionUpdate::Tool(tool.decode()?),
            Update::PermissionCancelled(cancellation) => {
                let cancellation = cancellation.decode()?;
                let named = match provider_context {
                    CancellationContext::AtFact(context) => cancellation.session_id() == context,
                    CancellationContext::Checkpoint => true,
                };
                if !named || cancellation.request().execution_id() != execution_id {
                    return Err(corrupt(
                        "cancellation belongs to a different session or invocation",
                    ));
                }
                ExecutionUpdate::PermissionCancelled(cancellation)
            }
            Update::ReviewDeclined {
                id,
                tool,
                reason,
                outcome,
                delivery,
                rule,
            } => {
                let outcome = match outcome {
                    DeclineOutcome::Continue => RefusalOutcome::Continue,
                    DeclineOutcome::EndTurn => RefusalOutcome::EndTurn,
                    DeclineOutcome::CloseSession => RefusalOutcome::CloseSession,
                };
                let rule = rule
                    .map(|rule| {
                        ApprovalRuleReference::new(
                            rule.rule_id,
                            rule.revision,
                            rule.granted_by.decode()?,
                        )
                        .map_err(corrupt)
                    })
                    .transpose()?;
                ExecutionUpdate::ReviewDeclined(
                    DeclinedReview::restore(
                        ReviewDeclineId::new(id).map_err(corrupt)?,
                        ReviewDecline::restore(
                            tool,
                            match reason {
                                DeclineReason::ToolNotReviewable => {
                                    ReviewDeclineReason::ToolNotReviewable
                                }
                                DeclineReason::UnreadableRequest => {
                                    ReviewDeclineReason::UnreadableRequest
                                }
                                DeclineReason::UnusableOptions => {
                                    ReviewDeclineReason::UnusableOptions
                                }
                            },
                            outcome,
                        )
                        .map_err(corrupt)?,
                        match delivery {
                            DeclineDelivery::Selected => ReviewDeclineStage::Selected,
                            DeclineDelivery::WriteConfirmed => ReviewDeclineStage::WriteConfirmed,
                            DeclineDelivery::WriteUnconfirmed => {
                                ReviewDeclineStage::WriteUnconfirmed
                            }
                            DeclineDelivery::WriteNotAttempted => {
                                ReviewDeclineStage::WriteNotAttempted
                            }
                        },
                        rule,
                    )
                    .map_err(corrupt)?,
                )
            }
            Update::PermissionRequested {
                id,
                tool,
                input,
                options,
                deadline,
            } => {
                validate_observation_id(&id).map_err(corrupt)?;
                let (tool_id, observation) = tool.decode_observation()?;
                ExecutionUpdate::PermissionRequested {
                    id: PermissionId::new(id).map_err(corrupt)?,
                    tool_id,
                    observation,
                    input: input.into(),
                    options: permissions::decode_choices(options)?,
                    deadline: deadline.and_then(restore_deadline),
                }
            }
            Update::DeadlineAnswered { id, effect, reason } => {
                validate_observation_id(&id).map_err(corrupt)?;
                let reason =
                    crate::domain::agent_execution::permissions::value_objects::checked_deadline_reason(
                        reason,
                    )
                    .map_err(corrupt)?;
                ExecutionUpdate::DeadlineAnswered {
                    id: PermissionId::new(id).map_err(corrupt)?,
                    effect: effect.into(),
                    reason,
                }
            }
            Update::QuestionClosed { id } => {
                validate_observation_id(&id).map_err(corrupt)?;
                ExecutionUpdate::QuestionClosed {
                    id: QuestionId::new(id).map_err(corrupt)?,
                }
            }
            Update::QuestionAsked {
                id,
                message,
                questions,
            } => {
                validate_observation_id(&id).map_err(corrupt)?;
                let questions = questions
                    .into_iter()
                    .map(|asked| {
                        let options = asked
                            .options
                            .into_iter()
                            .map(|option| {
                                AnswerOption::new(option.value, option.label, option.description)
                                    .map_err(corrupt)
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Question::new(
                            asked.key,
                            asked.prompt,
                            asked.header,
                            if asked.many {
                                AnswerShape::Many
                            } else {
                                AnswerShape::One
                            },
                            options,
                            asked.free_text_key,
                            asked.required,
                        )
                        .map_err(corrupt)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                ExecutionUpdate::QuestionAsked {
                    id: QuestionId::new(id).map_err(corrupt)?,
                    question: AgentQuestion::new(message, questions).map_err(corrupt)?,
                }
            }
        };
        let update = match (update, self.message_id) {
            (ExecutionUpdate::Message(chunk), Some(id)) => {
                let id = MessageId::new(id).map_err(corrupt)?;
                ExecutionUpdate::Message(chunk.with_message_id(id))
            }
            (update, None) => update,
            (_, Some(_)) => return Err(corrupt("message identity on a non-message observation")),
        };
        let event = ExecutionEvent::new(execution_id.clone(), update);
        let event = match self.native_hooks {
            Some(allowed) => event.with_native_hooks(allowed),
            None => event,
        };
        Ok(match self.at_ms {
            Some(at_ms) => event.with_observed_at_ms(at_ms),
            None => event,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub(super) enum Submission {
    Immediate,
    Queued,
    BoundarySteering,
    Steering,
}
impl From<SubmissionMode> for Submission {
    fn from(value: SubmissionMode) -> Self {
        match value {
            SubmissionMode::Immediate => Self::Immediate,
            SubmissionMode::Queued => Self::Queued,
            SubmissionMode::BoundarySteering => Self::BoundarySteering,
            SubmissionMode::Steering => Self::Steering,
        }
    }
}
impl From<Submission> for SubmissionMode {
    fn from(value: Submission) -> Self {
        match value {
            Submission::Immediate => Self::Immediate,
            Submission::Queued => Self::Queued,
            Submission::BoundarySteering => Self::BoundarySteering,
            Submission::Steering => Self::Steering,
        }
    }
}

#[cfg(test)]
mod borrowed_event_tests {
    use super::*;
    #[test]
    fn borrowed_text_and_thought_use_the_same_owned_codec_bytes_and_roundtrip() {
        for (name, chunk) in [
            ("Text", MessageChunk::text("π \"line\\\n")),
            ("Thought", MessageChunk::thought("🙂\t")),
            ("Text", MessageChunk::text("")),
        ] {
            let event = ExecutionEvent::new(
                ExecutionId::new("output").unwrap(),
                ExecutionUpdate::Message(chunk),
            );
            let borrowed = Event::from(&event);
            let bytes = serde_json::to_vec(&borrowed).unwrap();
            let owned: Event = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(serde_json::to_vec(&owned).unwrap(), bytes);
            let ExecutionUpdate::Message(chunk) = event.update() else {
                unreachable!()
            };
            let expected = format!(
                r#"{{"message_id":null,"execution_id":"output","update":{{"{name}":{}}}}}"#,
                serde_json::to_string(chunk.as_str()).unwrap()
            );
            assert_eq!(bytes, expected.as_bytes());
            let decoded = owned
                .decode(
                    CancellationContext::AtFact(
                        &crate::domain::agent_execution::sessions::ExecutionSessionId::new(
                            "remote",
                        )
                        .unwrap(),
                    ),
                    event.execution_id(),
                )
                .unwrap();
            assert_eq!(decoded, event);
        }
    }

    fn tool_event() -> ExecutionEvent {
        ExecutionEvent::new(
            ExecutionId::new("output").unwrap(),
            ExecutionUpdate::Tool(crate::domain::agent_execution::tools::ToolCallUpdate::new(
                crate::domain::agent_execution::tools::ToolCallId::new("tool-1").unwrap(),
                None,
                None,
                None,
                None,
                None,
            )),
        )
    }

    fn round_trip(event: &ExecutionEvent) -> (Vec<u8>, ExecutionEvent) {
        let bytes = serde_json::to_vec(&Event::from(event)).unwrap();
        let owned: Event = serde_json::from_slice(&bytes).unwrap();
        let decoded = owned
            .decode(
                CancellationContext::AtFact(
                    &crate::domain::agent_execution::sessions::ExecutionSessionId::new("remote")
                        .unwrap(),
                ),
                event.execution_id(),
            )
            .unwrap();
        (bytes, decoded)
    }

    #[test]
    fn a_tool_event_keeps_the_hooks_choice_it_was_launched_with() {
        for allowed in [false, true] {
            let event = tool_event().with_native_hooks(allowed);
            let (_, decoded) = round_trip(&event);
            assert_eq!(decoded.native_hooks(), Some(allowed));
            assert_eq!(decoded, event);
        }
        let (bytes, decoded) = round_trip(&tool_event());
        assert!(!String::from_utf8(bytes).unwrap().contains("native_hooks"));
        assert_eq!(decoded.native_hooks(), None);
    }

    #[test]
    fn a_tool_event_keeps_the_time_it_was_received() {
        let event = tool_event().with_observed_at_ms(1_700_000_000_123);
        let (bytes, decoded) = round_trip(&event);
        assert!(String::from_utf8(bytes)
            .unwrap()
            .contains(r#""at_ms":1700000000123"#));
        assert_eq!(decoded.observed_at_ms(), Some(1_700_000_000_123));
        assert_eq!(decoded, event);
        let (bytes, decoded) = round_trip(&tool_event());
        assert!(!String::from_utf8(bytes).unwrap().contains("at_ms"));
        assert_eq!(decoded.observed_at_ms(), None);
    }

    #[test]
    fn a_deadline_and_its_answer_round_trip_and_a_corrupt_deadline_is_skipped() {
        use crate::application::agent_execution::tools::ToolReviewInput;
        use crate::domain::agent_execution::permissions::{
            PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
            PermissionOption, PermissionOptionId, PermissionOptions, PermissionReviewDeadline,
            PermissionScope, PermissionToolClass, STANDARD_ALLOW_REASON, STANDARD_DENY_REASON,
            STANDARD_STOP_REASON,
        };
        use crate::domain::agent_execution::tools::{ToolCallId, ToolObservation};
        use std::time::Duration;

        let deadline = PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            STANDARD_DENY_REASON,
        )
        .unwrap()
        .stopping()
        .promise(1_700_000_030_000)
        .unwrap();
        let options = PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("cancel").unwrap(),
                "Cancel",
                PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap();
        let event = ExecutionEvent::new(
            ExecutionId::new("output").unwrap(),
            ExecutionUpdate::PermissionRequested {
                id: PermissionId::new("permission").unwrap(),
                tool_id: ToolCallId::new("tool").unwrap(),
                observation: ToolObservation::default(),
                input: ToolReviewInput {
                    name: "Write".into(),
                    arguments_json: "{}".into(),
                },
                options,
                deadline: Some(deadline.clone()),
            },
        );
        let borrowed = Event::from(&event);
        let bytes = serde_json::to_vec(&borrowed).unwrap();
        assert!(bytes.windows(8).any(|window| window == b"deadline"));
        assert!(std::str::from_utf8(&bytes).unwrap().contains("stops_turn"));
        let owned: Event = serde_json::from_slice(&bytes).unwrap();
        let decoded = owned
            .decode(
                CancellationContext::AtFact(
                    &crate::domain::agent_execution::sessions::ExecutionSessionId::new("remote")
                        .unwrap(),
                ),
                event.execution_id(),
            )
            .unwrap();
        let ExecutionUpdate::PermissionRequested {
            deadline: restored, ..
        } = decoded.update()
        else {
            panic!("review");
        };
        let restored = restored
            .as_ref()
            .expect("save and restore keep the deadline");
        assert_eq!(restored.class(), PermissionToolClass::Other);
        assert_eq!(restored.wait(), Duration::from_secs(30));
        assert_eq!(restored.effect(), PermissionEffect::Deny);
        assert_eq!(restored.reason(), STANDARD_STOP_REASON);
        assert!(restored.stops_turn());
        assert_eq!(restored.due_at_ms(), Some(1_700_000_030_000));
        assert!(!restored.armed(), "a restored deadline is not a promise");
        assert!(deadline.armed());
        // The recorded decision matches, so a live save can fold this review.
        // Armed stays out of that equality: restore must not promise a decision.
        assert_eq!(decoded, event);

        let answered = ExecutionEvent::new(
            ExecutionId::new("output").unwrap(),
            ExecutionUpdate::DeadlineAnswered {
                id: PermissionId::new("permission").unwrap(),
                effect: PermissionEffect::Deny,
                reason: deadline.recorded_reason(),
            },
        );
        let answered_owned: Event =
            serde_json::from_slice(&serde_json::to_vec(&Event::from(&answered)).unwrap()).unwrap();
        assert_eq!(
            answered_owned
                .decode(
                    CancellationContext::AtFact(
                        &crate::domain::agent_execution::sessions::ExecutionSessionId::new(
                            "remote"
                        )
                        .unwrap(),
                    ),
                    answered.execution_id(),
                )
                .unwrap(),
            answered
        );

        let blank = SavedDeadline {
            class: SavedClass::ReadOnly,
            wait_ms: 8_000,
            effect: SavedEffect::Allow,
            reason: " ".into(),
            stops_turn: false,
            due_at_ms: None,
        };
        assert!(restore_deadline(blank).is_none());
        let zero = SavedDeadline {
            class: SavedClass::Other,
            wait_ms: 0,
            effect: SavedEffect::Deny,
            reason: STANDARD_DENY_REASON.into(),
            stops_turn: false,
            due_at_ms: None,
        };
        assert!(restore_deadline(zero).is_none());
        let stopping_allow = SavedDeadline {
            class: SavedClass::ReadOnly,
            wait_ms: 8_000,
            effect: SavedEffect::Allow,
            reason: STANDARD_ALLOW_REASON.into(),
            stops_turn: true,
            due_at_ms: Some(1),
        };
        assert!(restore_deadline(stopping_allow).is_none());
        let kept = restore_deadline(saved_deadline(&deadline)).unwrap();
        assert_eq!(kept.reason(), STANDARD_STOP_REASON);
        assert!(!kept.armed());
    }
}
