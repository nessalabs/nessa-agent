//! Version 1 semantic fact bodies using the existing typed snapshot field mappings.
//! The physical frame and the application fold are separate boundaries.

use super::{
    cancellation::Cancellation,
    errors::{Outcome, SavedError},
    queue_order::QueueEvent,
    records::{Acknowledgement, Event, Metadata, Provider},
    scheduling::SchedulingEvent,
    settlement::Settlement,
    tools::corrupt,
};
use crate::{
    application::agent_execution::{
        providers::ExecutionReport,
        sessions::{ProviderContext, SessionChange, StorageError},
    },
    domain::agent_execution::{
        executions::ExecutionId,
        sessions::{ExecutionSessionId, SessionId},
    },
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum WireChange<E = Event> {
    Opened {
        id: String,
        provider: Provider,
        context: Option<String>,
    },
    InputAccepted {
        metadata: Box<Metadata>,
        scheduling: Vec<SchedulingEvent>,
    },
    QueueDecision(QueueEvent),
    SchedulingTransition {
        execution_id: String,
        event: SchedulingEvent,
    },
    ProviderObservation(E),
    ReceiptUpdated {
        execution_id: String,
        before: Acknowledgement,
        after: Acknowledgement,
    },
    StopDecision {
        execution_id: String,
        event: Cancellation,
    },
    ProviderReport {
        execution_id: String,
        report: Box<Settlement>,
        local_stop: Option<Cancellation>,
    },
    LocalSettlement {
        execution_id: String,
        before: Option<Result<Outcome, SavedError>>,
        after: Result<Outcome, SavedError>,
        local_outcome: Option<Outcome>,
    },
    ProviderContext {
        before: Option<String>,
        after: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBatch<C = WireChange> {
    changes: Vec<C>,
}

fn encode_context(value: &ProviderContext) -> Option<String> {
    value.recorded().map(|id| id.as_str().into())
}

fn decode_context(value: Option<String>) -> Result<ProviderContext, StorageError> {
    value
        .map(ExecutionSessionId::new)
        .transpose()
        .map_err(corrupt)
        .map(|value| value.map_or(ProviderContext::Absent, ProviderContext::Recorded))
}

fn encode_result(
    value: &Result<
        crate::domain::agent_execution::executions::ExecutionOutcome,
        crate::application::agent_execution::agents::AgentError,
    >,
) -> Result<Outcome, SavedError> {
    value.clone().map(Into::into).map_err(Into::into)
}

impl<'a> From<&'a SessionChange> for WireChange<Event<&'a str>> {
    fn from(value: &'a SessionChange) -> Self {
        match value {
            SessionChange::Opened {
                id,
                provider,
                context,
            } => Self::Opened {
                id: id.as_str().into(),
                provider: Provider {
                    name: provider.name().into(),
                    model_id: provider.model_id().into(),
                    context: provider.context().into(),
                },
                context: encode_context(context),
            },
            SessionChange::InputAccepted(record) => Self::InputAccepted {
                metadata: Box::new(Metadata::from(record.as_ref())),
                scheduling: record
                    .scheduling
                    .iter()
                    .cloned()
                    .map(SchedulingEvent::from)
                    .collect(),
            },
            SessionChange::QueueDecision(decision) => Self::QueueDecision(decision.into()),
            SessionChange::SchedulingTransition {
                execution_id,
                event,
            } => Self::SchedulingTransition {
                execution_id: execution_id.as_str().into(),
                event: event.clone().into(),
            },
            SessionChange::ProviderObservation(event) => Self::ProviderObservation(event.into()),
            SessionChange::ReceiptUpdated {
                execution_id,
                before,
                after,
            } => Self::ReceiptUpdated {
                execution_id: execution_id.as_str().into(),
                before: before.into(),
                after: after.into(),
            },
            SessionChange::StopDecision {
                execution_id,
                event,
            } => Self::StopDecision {
                execution_id: execution_id.as_str().into(),
                event: event.into(),
            },
            SessionChange::ProviderReport {
                execution_id,
                report,
                local_stop,
            } => Self::ProviderReport {
                execution_id: execution_id.as_str().into(),
                report: Box::new(report.clone().into()),
                local_stop: local_stop.as_ref().map(Into::into),
            },
            SessionChange::LocalSettlement {
                execution_id,
                before,
                after,
                local_outcome,
            } => Self::LocalSettlement {
                execution_id: execution_id.as_str().into(),
                before: before.as_ref().map(encode_result),
                after: encode_result(after),
                local_outcome: local_outcome.map(Into::into),
            },
            SessionChange::ProviderContext { before, after } => Self::ProviderContext {
                before: encode_context(before),
                after: encode_context(after),
            },
        }
    }
}

impl TryFrom<WireChange> for SessionChange {
    type Error = StorageError;

    fn try_from(value: WireChange) -> Result<Self, Self::Error> {
        Ok(match value {
            WireChange::Opened {
                id,
                provider,
                context,
            } => Self::Opened {
                id: SessionId::new(id).map_err(corrupt)?,
                provider: provider.decode()?,
                context: decode_context(context)?,
            },
            WireChange::InputAccepted {
                metadata,
                scheduling,
            } => {
                let mut record = metadata.decode()?;
                record.scheduling = scheduling
                    .into_iter()
                    .map(SchedulingEvent::decode)
                    .collect::<Result<_, _>>()?;
                Self::InputAccepted(Box::new(record))
            }
            WireChange::QueueDecision(decision) => Self::QueueDecision(decision.decode()?),
            WireChange::SchedulingTransition {
                execution_id,
                event,
            } => Self::SchedulingTransition {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                event: event.decode()?,
            },
            WireChange::ProviderObservation(_) => {
                return Err(corrupt("provider observation requires folded context"));
            }
            WireChange::ReceiptUpdated {
                execution_id,
                before,
                after,
            } => Self::ReceiptUpdated {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                before: before.try_into()?,
                after: after.try_into()?,
            },
            WireChange::StopDecision {
                execution_id,
                event,
            } => Self::StopDecision {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                event: event.decode()?,
            },
            WireChange::ProviderReport {
                execution_id,
                report,
                local_stop,
            } => Self::ProviderReport {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                report: ExecutionReport::try_from(*report)?,
                local_stop: local_stop.map(Cancellation::decode).transpose()?,
            },
            WireChange::LocalSettlement {
                execution_id,
                before,
                after,
                local_outcome,
            } => Self::LocalSettlement {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                before: before.map(super::errors::decode_result).transpose()?,
                after: super::errors::decode_result(after)?,
                local_outcome: local_outcome.map(Into::into),
            },
            WireChange::ProviderContext { before, after } => Self::ProviderContext {
                before: decode_context(before)?,
                after: decode_context(after)?,
            },
        })
    }
}

pub(crate) fn encode_change(change: &SessionChange) -> Result<Vec<u8>, StorageError> {
    serde_json::to_vec(&WireChange::from(change)).map_err(corrupt)
}

fn decode_wire_change(
    wire: WireChange,
    context: &mut ProviderContext,
) -> Result<SessionChange, StorageError> {
    match wire {
        WireChange::ProviderObservation(event) => {
            let provider_context = context
                .recorded()
                .ok_or_else(|| corrupt("provider observation has no provider context"))?;
            let execution_id = ExecutionId::new(&event.execution_id).map_err(corrupt)?;
            Ok(SessionChange::ProviderObservation(
                event.decode(provider_context, &execution_id)?,
            ))
        }
        other => {
            let change: SessionChange = other.try_into()?;
            match &change {
                SessionChange::Opened { context: next, .. }
                | SessionChange::ProviderContext { after: next, .. } => {
                    *context = next.clone();
                }
                _ => {}
            }
            Ok(change)
        }
    }
}

pub(super) fn decode_change(
    bytes: &[u8],
    context: &ProviderContext,
) -> Result<SessionChange, StorageError> {
    super::decode::preflight_semantic(bytes)?;
    let wire: WireChange = serde_json::from_slice(bytes).map_err(corrupt)?;
    decode_wire_change(wire, &mut context.clone())
}

pub(crate) fn encode_batch(changes: &[SessionChange]) -> Result<Vec<u8>, StorageError> {
    match changes {
        [] => Err(corrupt("empty semantic batch")),
        [only] => encode_change(only),
        _ => serde_json::to_vec(&WireBatch {
            changes: changes.iter().map(WireChange::from).collect(),
        })
        .map_err(corrupt),
    }
}

pub(crate) fn decode_batch(
    bytes: &[u8],
    grouped: bool,
    context: &ProviderContext,
) -> Result<Vec<SessionChange>, StorageError> {
    if !grouped {
        return decode_change(bytes, context).map(|change| vec![change]);
    }
    super::decode::preflight_semantic_batch(bytes)?;
    let batch: WireBatch = serde_json::from_slice(bytes).map_err(corrupt)?;
    if batch.changes.len() < 2 {
        return Err(corrupt("atomic transition needs multiple changes"));
    }
    let mut context = context.clone();
    batch
        .changes
        .into_iter()
        .map(|wire| decode_wire_change(wire, &mut context))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
            permissions::ActionContext,
            providers::ProviderIdentity,
            sessions::{InvocationRecord, SubmissionAcknowledgement},
        },
        domain::agent_execution::{
            executions::{MessageChunk, MessageId},
            permissions::{
                ReviewDecline, ReviewDeclineId, ReviewDeclineObservation, ReviewDeclineReason,
                ReviewDeclineStage,
            },
            prompts::{
                AppModelContext, ImageReference, McpAppSource, MessageSender, PromptText,
                UserMessage,
            },
            tools::{McpTool, ToolCallId},
        },
        domain::common::value_objects::{ImageMediaType, Sha256Digest},
    };

    #[test]
    fn admission_round_trip_reuses_the_existing_metadata_mapping() {
        let id = ExecutionId::new("one").unwrap();
        let record = InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: id,
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        let change = SessionChange::InputAccepted(Box::new(record.clone()));
        let bytes = encode_change(&change).unwrap();
        let decoded = decode_change(&bytes, &ProviderContext::Absent).unwrap();
        assert!(matches!(decoded, SessionChange::InputAccepted(next) if *next == record));
    }

    #[test]
    fn observation_requires_the_recorded_provider_context() {
        let id = ExecutionId::new("one").unwrap();
        let event = ExecutionEvent::new(id, ExecutionUpdate::Message(MessageChunk::text("chunk")));
        let bytes = encode_change(&SessionChange::ProviderObservation(event.clone())).unwrap();
        assert!(matches!(
            decode_change(&bytes, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        assert!(matches!(
            decode_change(&bytes, &context).unwrap(),
            SessionChange::ProviderObservation(next) if next == event
        ));
    }

    #[test]
    fn a_tool_observation_keeps_its_mcp_identity_and_structured_result_through_storage() {
        use crate::domain::agent_execution::tools::{
            McpTool, ToolCallId, ToolCallUpdate, ToolContent, MAX_MCP_NAME_BYTES,
        };
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let tool = |update| {
            ExecutionEvent::new(
                ExecutionId::new("one").unwrap(),
                ExecutionUpdate::Tool(update),
            )
        };
        let round_trip = |event: &ExecutionEvent| {
            let bytes = encode_change(&SessionChange::ProviderObservation(event.clone())).unwrap();
            let decoded = decode_change(&bytes, &context).unwrap();
            assert!(matches!(decoded, SessionChange::ProviderObservation(next) if next == *event));
            String::from_utf8(bytes).unwrap()
        };
        let bare =
            || ToolCallUpdate::new(ToolCallId::new("t").unwrap(), None, None, None, None, None);
        let named = tool(
            bare()
                .with_content(vec![
                    ToolContent::text("rows: 2"),
                    ToolContent::structured(r#"{"rows":2}"#).unwrap(),
                ])
                .with_mcp_tool(McpTool::new("charts", "show").unwrap()),
        );
        let text = round_trip(&named);
        // A tool no update named is written as before, without the field.
        assert!(!round_trip(&tool(bare())).contains("mcp_tool"));
        // A saved identity or result the domain would refuse is corrupt, and a
        // name past its bound is refused before it is decoded.
        // Which refuses is part of the rule: past its bound, or with a field
        // the record does not have, before anything is decoded; a name the
        // domain will not keep, when it is constructed.
        for (to, refusal) in [
            (r#""server":"two words""#.to_owned(), "InvalidMcpToolName"),
            (
                format!(r#""server":"{}""#, "s".repeat(MAX_MCP_NAME_BYTES + 1)),
                "exceeds decoded string limit",
            ),
            (
                r#""server":"charts","ui":"x""#.to_owned(),
                "unknown field for its schema",
            ),
        ] {
            let corrupt = text.replacen(r#""server":"charts""#, &to, 1);
            assert_ne!(corrupt, text);
            match decode_change(corrupt.as_bytes(), &context) {
                Err(StorageError::Corrupt(message)) => {
                    assert!(message.contains(refusal), "{to}: {message}")
                }
                other => panic!("{to}: {other:?}"),
            }
        }
        // A saved structured result that is not JSON is refused on restore by
        // the domain's constructor, not carried on as a structured value.
        let not_json = text.replacen(r#"{\"rows\":2}"#, "not json", 1);
        assert_ne!(not_json, text);
        match decode_change(not_json.as_bytes(), &context) {
            Err(StorageError::Corrupt(message)) => {
                assert!(message.contains("InvalidStructuredResult"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        let oversize = text.replacen(
            r#"{\"rows\":2}"#,
            &"a".repeat(crate::domain::agent_execution::tools::MAX_STRUCTURED_RESULT_BYTES + 1),
            1,
        );
        assert_ne!(oversize, text);
        // Refused at its bound before it is decoded, not after.
        match decode_change(oversize.as_bytes(), &context) {
            Err(StorageError::Corrupt(message)) => {
                assert!(
                    message.contains("exceeds decoded string limit"),
                    "{message}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn semantic_observation_refuses_an_empty_message_identity() {
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let event = ExecutionEvent::new(
            ExecutionId::new("one").unwrap(),
            ExecutionUpdate::Message(
                MessageChunk::text("chunk").with_message_id(MessageId::new("valid").unwrap()),
            ),
        );
        let bytes = encode_change(&SessionChange::ProviderObservation(event)).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["ProviderObservation"]["message_id"] = "".into();
        assert!(matches!(
            decode_change(&serde_json::to_vec(&value).unwrap(), &context),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn semantic_admission_restores_image_references_through_domain_rules() {
        let id = ExecutionId::new("one").unwrap();
        let image =
            ImageReference::new(Sha256Digest::from_bytes([1; 32]), ImageMediaType::Png, 1).unwrap();
        let record = InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: id,
                user_message: UserMessage::new(None, vec![image], Vec::new()).unwrap(),
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        let bytes = encode_change(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        assert!(matches!(
            decode_change(&bytes, &ProviderContext::Absent).unwrap(),
            SessionChange::InputAccepted(next) if *next == record
        ));
        let valid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (field, replacement) in [
            ("digest", serde_json::Value::from("sha256:00")),
            (
                "digest",
                serde_json::Value::from(format!("sha256:{}", "AB".repeat(32))),
            ),
            ("media_type", serde_json::Value::from("image/svg+xml")),
            ("size", serde_json::Value::from(0)),
        ] {
            let mut invalid = valid.clone();
            invalid["InputAccepted"]["metadata"]["user_images"][0][field] = replacement;
            assert!(
                matches!(
                    decode_change(
                        &serde_json::to_vec(&invalid).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{field}"
            );
        }
    }

    #[test]
    fn semantic_admission_restores_who_wrote_it_and_what_apps_gave_with_it() {
        let app = |tool_id: &str| {
            McpAppSource::new(
                ExecutionId::new("turn-0").unwrap(),
                ToolCallId::new(tool_id).unwrap(),
                McpTool::new("charts", "plot").unwrap(),
            )
            .unwrap()
        };
        let message = UserMessage::text_only(PromptText::new("plot May").unwrap())
            .sent_by(MessageSender::App(app("call-1")))
            .with_app_model_context(vec![
                AppModelContext::new(app("call-1"), "update-1", Some("zoomed".into()), None)
                    .unwrap()
                    .unwrap(),
                AppModelContext::new(
                    app("call-2"),
                    "update-1",
                    None,
                    Some(r#"{"month":5}"#.into()),
                )
                .unwrap()
                .unwrap(),
            ])
            .unwrap();
        let record = InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("one").unwrap(),
                user_message: message,
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        let bytes = encode_change(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        assert!(matches!(
            decode_change(&bytes, &ProviderContext::Absent).unwrap(),
            SessionChange::InputAccepted(next) if *next == record
        ));
        // Each part is rebuilt through the domain: what it would refuse is a
        // corrupt record, not a message the agent is handed.
        let valid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for (path, replacement) in [
            (
                &["user_app", "server"][..],
                serde_json::Value::from("two words"),
            ),
            (&["user_app", "tool_id"][..], serde_json::Value::from(" ")),
            (&["user_app", "extra"][..], serde_json::Value::from("x")),
            (
                &["user_app_model_context", "1", "structured_content"][..],
                "[5]".into(),
            ),
            (
                &["user_app_model_context", "0", "text"][..],
                serde_json::Value::Null,
            ),
            // Saved only as none, never as empty: one empty was changed.
            // On the one with structure too, where an empty text read as
            // none would leave a context standing.
            (&["user_app_model_context", "1", "text"][..], "".into()),
            (&["user_app_model_context", "0", "update"][..], " ".into()),
            // An app of the record's own turn: no earlier tool call drew it.
            (&["user_app", "execution_id"][..], "one".into()),
            (
                &["user_app_model_context", "1", "app", "execution_id"][..],
                "one".into(),
            ),
            (
                &["user_app_model_context", "0", "text"][..],
                "x".repeat(AppModelContext::MAX_BYTES + 1).into(),
            ),
        ] {
            let mut invalid = valid.clone();
            let mut at = &mut invalid["InputAccepted"]["metadata"];
            for key in path {
                at = match key.parse::<usize>() {
                    Ok(index) => &mut at[index],
                    Err(_) => &mut at[*key],
                };
            }
            *at = replacement;
            assert!(
                matches!(
                    decode_change(
                        &serde_json::to_vec(&invalid).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{path:?}"
            );
        }
        // Five contexts are refused before a fifth is built.
        let mut five = valid.clone();
        let contexts = five["InputAccepted"]["metadata"]["user_app_model_context"]
            .as_array_mut()
            .unwrap();
        let first = contexts[0].clone();
        contexts.extend([first.clone(), first.clone(), first]);
        assert!(matches!(
            decode_change(
                &serde_json::to_vec(&five).unwrap(),
                &ProviderContext::Absent
            ),
            Err(StorageError::Corrupt(_))
        ));
        // A context missing either part's key is not of this shape either,
        // not read as that part being none.
        for part in ["text", "structured_content"] {
            let mut partial = valid.clone();
            partial["InputAccepted"]["metadata"]["user_app_model_context"][1]
                .as_object_mut()
                .unwrap()
                .remove(part);
            assert!(
                matches!(
                    decode_change(
                        &serde_json::to_vec(&partial).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{part}"
            );
        }
        // A record without either field is not of this shape: corrupt, not
        // read as the person's with nothing given (no older reader is kept).
        for field in ["user_app", "user_app_model_context"] {
            let mut older = valid.clone();
            older["InputAccepted"]["metadata"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                matches!(
                    decode_change(
                        &serde_json::to_vec(&older).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{field}"
            );
        }
        // A person's message says so: `user_app` is null.
        let person = UserMessage::text_only(PromptText::new("mine").unwrap());
        let mut record = record;
        record.request.user_message = person;
        let bytes = encode_change(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            saved["InputAccepted"]["metadata"]["user_app"],
            serde_json::Value::Null
        );
        assert!(matches!(
            decode_change(&bytes, &ProviderContext::Absent).unwrap(),
            SessionChange::InputAccepted(next) if *next == record
        ));
    }

    #[test]
    fn semantic_decline_stages_preserve_identity_and_order() {
        let id = ExecutionId::new("one").unwrap();
        let mut events = Vec::new();
        for (decline_id, terminal) in [
            ("1", ReviewDeclineStage::WriteConfirmed),
            ("2", ReviewDeclineStage::WriteUnconfirmed),
        ] {
            let selected = ReviewDeclineObservation::selected(
                ReviewDeclineId::new(decline_id).unwrap(),
                ReviewDecline::new(Some("Read"), ReviewDeclineReason::ToolNotReviewable),
            );
            events.push(ExecutionEvent::new(
                id.clone(),
                ExecutionUpdate::ReviewDeclined(selected.clone()),
            ));
            events.push(ExecutionEvent::new(
                id.clone(),
                ExecutionUpdate::ReviewDeclined(selected.advance(terminal).unwrap()),
            ));
        }
        let changes: Vec<_> = events
            .iter()
            .cloned()
            .map(SessionChange::ProviderObservation)
            .collect();
        let bytes = encode_batch(&changes).unwrap();
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let restored = decode_batch(&bytes, true, &context).unwrap();
        let restored: Vec<_> = restored
            .into_iter()
            .map(|change| match change {
                SessionChange::ProviderObservation(event) => event,
                _ => panic!("decoded fact remains an observation"),
            })
            .collect();
        assert_eq!(restored, events);
    }

    #[test]
    fn unknown_required_wire_fields_are_refused() {
        let change = SessionChange::Opened {
            id: SessionId::new("conversation").unwrap(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let bytes = encode_change(&change).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["Opened"]["unrecognized"] = true.into();
        let changed = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            decode_change(&changed, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn oversized_observation_text_is_refused_by_shared_preflight() {
        let body = serde_json::json!({
            "ProviderObservation": {
                "message_id": null,
                "execution_id": "one",
                "update": {"Text": "x".repeat(crate::application::agent_execution::executions::ExecutionEvent::MAX_MESSAGE_CHUNK_BYTES + 1)}
            }
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        assert!(matches!(
            decode_change(&bytes, &context),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn grouped_changes_update_context_before_decoding_later_observations() {
        let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let event = ExecutionEvent::new(
            ExecutionId::new("one").unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("chunk")),
        );
        let changes = vec![
            SessionChange::Opened {
                id: SessionId::new("conversation").unwrap(),
                provider,
                context: ProviderContext::Absent,
            },
            SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: context,
            },
            SessionChange::ProviderObservation(event.clone()),
        ];
        let bytes = encode_batch(&changes).unwrap();
        let decoded = decode_batch(&bytes, true, &ProviderContext::Absent).unwrap();
        assert!(matches!(
            &decoded[2],
            SessionChange::ProviderObservation(next) if next == &event
        ));
        assert!(matches!(
            decode_batch(&bytes, false, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn a_group_with_only_one_change_is_not_an_atomic_transition() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "changes": [{"Opened": {
                "id": "conversation",
                "provider": {"name": "fixture", "model_id": "model", "context": "workspace"},
                "context": null
            }}]
        }))
        .unwrap();
        assert!(matches!(
            decode_batch(&bytes, true, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
    }
}
