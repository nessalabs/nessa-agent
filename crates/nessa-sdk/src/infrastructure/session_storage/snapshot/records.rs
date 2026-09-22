use super::{
    cancellation::Cancellation as InvocationCancellation,
    errors::{Outcome, SavedError},
    permissions::{self, Actor, Cancellation, Choice, Input},
    settlement::Settlement,
    tools::{corrupt, Tool},
};
use crate::application::agent_execution::{
    executions::{
        limits::validate_observation_id, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
        SubmissionMode,
    },
    providers::ProviderIdentity,
    sessions::storage::{InvocationRecord, StorageError},
};
use crate::domain::{
    agent_execution::{
        executions::{ExecutionId, MessageChunk, MessageId, MessageKind},
        permissions::PermissionId,
        prompts::{ImageReference, LinkedFile, PromptText, UserMessage},
        questions::{AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId},
        sessions::ExecutionSessionId,
    },
    common::value_objects::{ImageMediaType, Sha256Digest},
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
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Metadata {
    pub(super) target_event_offset: Option<usize>,
    pub(super) submission: Submission,
    pub(super) execution_id: String,
    /// Empty for a message of images or files alone.
    pub(super) user_message: String,
    pub(super) user_images: Vec<Image>,
    /// Absent in a journal written before a message could point at files, and
    /// empty is what such a message meant: no message could name one.
    ///
    /// This is not a compatibility reader kept alongside a current one. There
    /// is one contract, and under it an older record says truthfully that its
    /// message named no files — the same reading the six `Option` fields
    /// beside this one already take of their own absence. What the default
    /// must never do is invent a value a record could have meant something
    /// else by, which is why the path itself has none: a missing `path` is a
    /// corrupt record, not an empty one.
    #[serde(default)]
    pub(super) user_files: Vec<FileLink>,
    pub(super) estimated_input_tokens: u64,
    pub(super) reserved_output_tokens: u32,
    pub(super) actor: Actor,
    pub(super) provider_report: Option<Settlement>,
    pub(super) local_cancellation: Option<InvocationCancellation>,
    pub(super) local_outcome: Option<Outcome>,
    pub(super) cancellation: Option<InvocationCancellation>,
    pub(super) result: Option<Result<Outcome, SavedError>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Event {
    pub(super) message_id: Option<String>,
    pub(super) execution_id: String,
    pub(super) update: Update,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Update {
    Finished(Outcome),
    Text(String),
    Thought(String),
    Tool(Tool),
    PermissionCancelled(Cancellation),
    PermissionRequested {
        id: String,
        tool: Tool,
        input: Input,
        options: Vec<Choice>,
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
    pub(super) free_text: bool,
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
impl From<&InvocationRecord> for Metadata {
    fn from(value: &InvocationRecord) -> Self {
        Self {
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
            user_files: value
                .request
                .user_message
                .files()
                .iter()
                .map(Into::into)
                .collect(),
            estimated_input_tokens: value.request.estimated_input_tokens,
            reserved_output_tokens: value.request.reserved_output_tokens,
            actor: (&value.actor).into(),
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
impl From<ExecutionEvent> for Event {
    fn from(event: ExecutionEvent) -> Self {
        Self {
            message_id: match event.update() {
                ExecutionUpdate::Message(chunk) => chunk.message_id().map(str::to_owned),
                _ => None,
            },
            execution_id: event.execution_id().as_str().into(),
            update: match event.into_update() {
                ExecutionUpdate::Finished(outcome) => Update::Finished(outcome.into()),
                ExecutionUpdate::Message(chunk) => match chunk.kind() {
                    MessageKind::Text => Update::Text(chunk.into_text()),
                    MessageKind::Thought => Update::Thought(chunk.into_text()),
                },
                ExecutionUpdate::Tool(tool) => Update::Tool((&tool).into()),
                ExecutionUpdate::PermissionCancelled(cancellation) => {
                    Update::PermissionCancelled((&cancellation).into())
                }
                ExecutionUpdate::PermissionRequested {
                    id,
                    tool_id,
                    observation,
                    input,
                    options,
                } => Update::PermissionRequested {
                    id: id.as_str().into(),
                    tool: Tool::observation(&tool_id, &observation),
                    input: (&input).into(),
                    options: permissions::choices(&options),
                },
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
                            free_text: asked.free_text(),
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
impl Provider {
    pub(super) fn decode(self) -> Result<ProviderIdentity, StorageError> {
        ProviderIdentity::new(self.name, self.model_id, self.context).map_err(corrupt)
    }
}
impl Metadata {
    pub(super) fn decode(self) -> Result<InvocationRecord, StorageError> {
        Ok(InvocationRecord {
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
                )
                .map_err(corrupt)?,
                estimated_input_tokens: self.estimated_input_tokens,
                reserved_output_tokens: self.reserved_output_tokens,
            },
            actor: self.actor.decode()?,
            events: Vec::new(),
            scheduling: Vec::new(),
            provider_report: self.provider_report.map(Into::into),
            local_cancellation: self
                .local_cancellation
                .map(InvocationCancellation::decode)
                .transpose()?,
            local_outcome: self.local_outcome.map(Into::into),
            cancellation: self
                .cancellation
                .map(InvocationCancellation::decode)
                .transpose()?,
            result: self
                .result
                .map(|result| result.map(Into::into).map_err(Into::into)),
        })
    }
}
impl Event {
    pub(super) fn decode(
        self,
        provider_session_id: &ExecutionSessionId,
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
                if cancellation.session_id() != provider_session_id
                    || cancellation.request().execution_id() != execution_id
                {
                    return Err(corrupt(
                        "cancellation belongs to a different session or invocation",
                    ));
                }
                ExecutionUpdate::PermissionCancelled(cancellation)
            }
            Update::PermissionRequested {
                id,
                tool,
                input,
                options,
            } => {
                validate_observation_id(&id).map_err(corrupt)?;
                let (tool_id, observation) = tool.decode_observation()?;
                ExecutionUpdate::PermissionRequested {
                    id: PermissionId::new(id).map_err(corrupt)?,
                    tool_id,
                    observation,
                    input: input.into(),
                    options: permissions::decode_choices(options)?,
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
                            asked.free_text,
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
        Ok(ExecutionEvent::new(execution_id.clone(), update))
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
