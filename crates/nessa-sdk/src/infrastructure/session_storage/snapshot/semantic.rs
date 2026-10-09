//! Semantic fact bodies. Each batch carries `schemaVersion`
//! ([`StorageError::SCHEMA_VERSION`]). The physical frame and the application
//! fold are separate boundaries.

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
        sessions::{ProviderContext, SessionChange, SessionSaveUnit, StorageError},
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
    #[serde(rename = "schemaVersion")]
    schema_version: u64,
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

pub(crate) fn encode_batch(changes: &[SessionChange]) -> Result<Vec<u8>, StorageError> {
    SessionSaveUnit::check_changes(changes)?;
    let bytes = serde_json::to_vec(&WireBatch {
        schema_version: StorageError::SCHEMA_VERSION,
        changes: changes.iter().map(WireChange::from).collect(),
    })
    .map_err(corrupt)?;
    super::super::save_group::validate_unit_payload(&bytes)?;
    super::decode::preflight_semantic_batch(bytes.as_slice())?;
    Ok(bytes)
}

pub(crate) fn decode_batch(
    bytes: &[u8],
    context: &ProviderContext,
) -> Result<Vec<SessionChange>, StorageError> {
    super::decode::preflight_semantic_batch(bytes)?;
    let batch: WireBatch = serde_json::from_slice(bytes).map_err(corrupt)?;
    let mut context = context.clone();
    let changes = batch
        .changes
        .into_iter()
        .map(|wire| decode_wire_change(wire, &mut context))
        .collect::<Result<Vec<_>, _>>()?;
    SessionSaveUnit::check_changes(&changes)?;
    Ok(changes)
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

    fn encode_one(change: &SessionChange) -> Result<Vec<u8>, StorageError> {
        encode_batch(std::slice::from_ref(change))
    }
    fn decode_one(bytes: &[u8], context: &ProviderContext) -> Result<SessionChange, StorageError> {
        let mut changes = decode_batch(bytes, context)?;
        assert_eq!(
            changes.len(),
            1,
            "single-unit fixture must contain one change"
        );
        Ok(changes.remove(0))
    }

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
        let bytes = encode_one(&change).unwrap();
        let decoded = decode_one(&bytes, &ProviderContext::Absent).unwrap();
        assert!(matches!(decoded, SessionChange::InputAccepted(next) if *next == record));
    }

    #[test]
    fn observation_requires_the_recorded_provider_context() {
        let id = ExecutionId::new("one").unwrap();
        let event = ExecutionEvent::new(id, ExecutionUpdate::Message(MessageChunk::text("chunk")));
        let bytes = encode_one(&SessionChange::ProviderObservation(event.clone())).unwrap();
        assert!(matches!(
            decode_one(&bytes, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        assert!(matches!(
            decode_one(&bytes, &context).unwrap(),
            SessionChange::ProviderObservation(next) if next == event
        ));
    }

    #[test]
    fn a_tool_observation_keeps_its_mcp_identity_and_structured_result_through_storage() {
        use crate::domain::agent_execution::tools::{
            McpCallArguments, McpTool, ToolCallId, ToolCallUpdate, ToolContent,
            MAX_MCP_ARGUMENTS_BYTES, MAX_MCP_NAME_BYTES,
        };
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let tool = |update| {
            ExecutionEvent::new(
                ExecutionId::new("one").unwrap(),
                ExecutionUpdate::Tool(update),
            )
        };
        let round_trip = |event: &ExecutionEvent| {
            let bytes = encode_one(&SessionChange::ProviderObservation(event.clone())).unwrap();
            let decoded = decode_one(&bytes, &context).unwrap();
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
        let with_arguments =
            tool(bare().with_mcp_arguments(McpCallArguments::new(r#"{"city":"Oslo"}"#).unwrap()));
        let arguments_text = round_trip(&with_arguments);
        assert!(arguments_text.contains(r#""mcp_arguments":"{\"city\":\"Oslo\"}""#));
        assert!(!round_trip(&tool(bare())).contains("mcp_arguments"));
        let over = arguments_text.replacen("Oslo", &"x".repeat(MAX_MCP_ARGUMENTS_BYTES), 1);
        assert_ne!(over, arguments_text);
        assert!(
            decode_one(over.as_bytes(), &context)
                .unwrap_err()
                .to_string()
                .contains("exceeds decoded string limit"),
            "{over}"
        );
        let not_an_object = arguments_text.replacen(r#"{\"city\":\"Oslo\"}"#, "[1]", 1);
        assert!(
            decode_one(not_an_object.as_bytes(), &context)
                .unwrap_err()
                .to_string()
                .contains("InvalidMcpCallArguments"),
            "{not_an_object}"
        );
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
            match decode_one(corrupt.as_bytes(), &context) {
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
        match decode_one(not_json.as_bytes(), &context) {
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
        match decode_one(oversize.as_bytes(), &context) {
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
        let bytes = encode_one(&SessionChange::ProviderObservation(event)).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["changes"][0]["ProviderObservation"]["message_id"] = "".into();
        assert!(matches!(
            decode_one(&serde_json::to_vec(&value).unwrap(), &context),
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
        let bytes = encode_one(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        assert!(matches!(
            decode_one(&bytes, &ProviderContext::Absent).unwrap(),
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
            invalid["changes"][0]["InputAccepted"]["metadata"]["user_images"][0][field] =
                replacement;
            assert!(
                matches!(
                    decode_one(
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
        let bytes = encode_one(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        assert!(matches!(
            decode_one(&bytes, &ProviderContext::Absent).unwrap(),
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
            (
                &["user_app_model_context", "0", "text"][..],
                "x".repeat(AppModelContext::MAX_BYTES + 1).into(),
            ),
        ] {
            let mut invalid = valid.clone();
            let mut at = &mut invalid["changes"][0]["InputAccepted"]["metadata"];
            for key in path {
                at = match key.parse::<usize>() {
                    Ok(index) => &mut at[index],
                    Err(_) => &mut at[*key],
                };
            }
            *at = replacement;
            assert!(
                matches!(
                    decode_one(
                        &serde_json::to_vec(&invalid).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{path:?}"
            );
        }
        // Five contexts are refused by the decoder, before a fifth is built,
        // not only by the message's own constructor after all five were.
        let mut five = valid.clone();
        let contexts = five["changes"][0]["InputAccepted"]["metadata"]["user_app_model_context"]
            .as_array_mut()
            .unwrap();
        let first = contexts[0].clone();
        contexts.extend([first.clone(), first.clone(), first]);
        assert!(matches!(
            decode_one(
                &serde_json::to_vec(&five).unwrap(),
                &ProviderContext::Absent
            ),
            Err(StorageError::Corrupt(message))
                if message.contains("journal collection exceeds decoding limit")
        ));
        // A context missing either part's key is not of this shape either,
        // not read as that part being none.
        for part in ["text", "structured_content"] {
            let mut partial = valid.clone();
            partial["changes"][0]["InputAccepted"]["metadata"]["user_app_model_context"][1]
                .as_object_mut()
                .unwrap()
                .remove(part);
            assert!(
                matches!(
                    decode_one(
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
            older["changes"][0]["InputAccepted"]["metadata"]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                matches!(
                    decode_one(
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
        let bytes = encode_one(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            saved["changes"][0]["InputAccepted"]["metadata"]["user_app"],
            serde_json::Value::Null
        );
        assert!(matches!(
            decode_one(&bytes, &ProviderContext::Absent).unwrap(),
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
        let restored = decode_batch(&bytes, &context).unwrap();
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
        let bytes = encode_one(&change).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["changes"][0]["Opened"]["unrecognized"] = true.into();
        let changed = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            decode_one(&changed, &ProviderContext::Absent),
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
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [body]
        }))
        .unwrap();
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        assert!(matches!(
            decode_one(&bytes, &context),
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
        let decoded = decode_batch(&bytes, &ProviderContext::Absent).unwrap();
        assert!(matches!(
            &decoded[2],
            SessionChange::ProviderObservation(next) if next == &event
        ));
        let bare = serde_json::to_vec(&WireChange::from(&changes[0])).unwrap();
        // A body with no batch envelope also has no schemaVersion, so it is
        // another version rather than a broken current record.
        assert_eq!(
            decode_batch(&bare, &ProviderContext::Absent),
            Err(StorageError::AnotherVersion { found: None })
        );
    }

    fn opened_change() -> SessionChange {
        SessionChange::Opened {
            id: SessionId::new("conversation").unwrap(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        }
    }

    #[test]
    fn a_semantic_batch_round_trips_and_carries_schema_version() {
        let change = opened_change();
        let bytes = encode_one(&change).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["schemaVersion"], StorageError::SCHEMA_VERSION);
        assert_eq!(
            decode_one(&bytes, &ProviderContext::Absent).unwrap(),
            change
        );
    }

    #[test]
    fn an_unmarked_record_is_another_version_and_is_not_read() {
        let change = opened_change();
        let bytes = encode_one(&change).unwrap();
        let mut saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        saved.as_object_mut().unwrap().remove("schemaVersion");
        // The pre-marker shape also lacks fields this build requires. The
        // marker is what the refusal names; the missing field is not read.
        saved["changes"][0]["Opened"]
            .as_object_mut()
            .unwrap()
            .remove("id");
        let old = serde_json::to_vec(&saved).unwrap();
        let kept = old.clone();
        assert_eq!(
            decode_batch(&old, &ProviderContext::Absent),
            Err(StorageError::AnotherVersion { found: None })
        );
        assert_eq!(old, kept);
    }

    #[test]
    fn a_future_record_is_another_version_and_is_not_read() {
        let bytes = encode_one(&opened_change()).unwrap();
        let mut saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let found = StorageError::SCHEMA_VERSION + 1;
        saved["schemaVersion"] = serde_json::json!(found);
        saved["notInThisShape"] = serde_json::json!(true);
        let future = serde_json::to_vec(&saved).unwrap();
        let kept = future.clone();
        assert_eq!(
            decode_batch(&future, &ProviderContext::Absent),
            Err(StorageError::AnotherVersion { found: Some(found) })
        );
        assert_eq!(future, kept);
        // Zero is a different unsigned integer, not this build's version.
        saved["schemaVersion"] = serde_json::json!(0);
        assert_eq!(
            decode_batch(
                &serde_json::to_vec(&saved).unwrap(),
                &ProviderContext::Absent
            ),
            Err(StorageError::AnotherVersion { found: Some(0) })
        );
    }

    #[test]
    fn a_record_that_is_not_an_object_is_corrupt() {
        for bytes in [b"[]".as_slice(), b"null", b"\"record\"", b"", b"{"] {
            assert!(
                matches!(
                    decode_batch(bytes, &ProviderContext::Absent),
                    Err(StorageError::Corrupt(_))
                ),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn a_current_record_with_a_broken_body_is_still_corrupt() {
        let bytes = encode_one(&opened_change()).unwrap();
        let mut saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        saved["changes"] = serde_json::json!("not a list");
        let broken = serde_json::to_vec(&saved).unwrap();
        assert!(matches!(
            decode_batch(&broken, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn a_marker_that_is_not_an_unsigned_integer_is_corrupt() {
        let bytes = encode_one(&opened_change()).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for marker in [
            serde_json::json!("1"),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(null),
        ] {
            let mut changed = saved.clone();
            changed["schemaVersion"] = marker;
            assert!(
                matches!(
                    decode_batch(
                        &serde_json::to_vec(&changed).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{changed}"
            );
        }
    }
}
