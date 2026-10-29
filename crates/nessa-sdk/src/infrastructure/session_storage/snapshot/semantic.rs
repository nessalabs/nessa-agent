//! Version 1 semantic fact bodies using the existing typed snapshot field mappings.
//! The physical frame and the application fold are separate boundaries.

use crate::domain::model_metadata::value_objects::EffortLevel;

use super::{
    artifacts::WireArtifact,
    cancellation::Cancellation,
    errors::{Outcome, SavedError},
    handoffs::WireHandoff,
    leases::WireLease,
    notes::{WireNote, WireNoteRetired},
    permissions::Actor,
    queue_order::QueueEvent,
    records::{Acknowledgement, CancellationContext, Event, Metadata, Provider},
    scheduling::SchedulingEvent,
    settlement::Settlement,
    tools::corrupt,
};
use crate::{
    application::agent_execution::{
        providers::{
            ApprovalMode, ExecutionReport, ModelChange, ModelChangeRecord, ModelChangeTiming,
            ModelConfiguration, SessionConfigurationKind, SessionConfigurationRecord,
        },
        sessions::{ProviderContext, SessionChange, SessionSaveUnit, StorageError},
    },
    domain::agent_execution::{
        executions::ExecutionId,
        sessions::{ExecutionSessionId, SessionId},
    },
};
use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::fmt;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedConfigurationChange {
    before: Option<SavedModelConfiguration>,
    after: SavedModelConfiguration,
    actor: Actor,
    kind: String,
}
impl From<&SessionConfigurationRecord> for SavedConfigurationChange {
    fn from(record: &SessionConfigurationRecord) -> Self {
        Self {
            before: record.before().map(Into::into),
            after: record.after().into(),
            actor: record.actor().into(),
            kind: match record.kind() {
                SessionConfigurationKind::Model => "model",
                SessionConfigurationKind::ApprovalMode => "approvalMode",
                SessionConfigurationKind::EffortLevel => "effortLevel",
            }
            .into(),
        }
    }
}
impl SavedConfigurationChange {
    pub(super) fn decode(self) -> Result<SessionConfigurationRecord, StorageError> {
        let kind = match self.kind.as_str() {
            "model" => SessionConfigurationKind::Model,
            "approvalMode" => SessionConfigurationKind::ApprovalMode,
            "effortLevel" => SessionConfigurationKind::EffortLevel,
            _ => return Err(corrupt("unknown configuration change kind")),
        };
        Ok(SessionConfigurationRecord::new(
            self.before
                .map(SavedModelConfiguration::decode)
                .transpose()?,
            self.after.decode()?,
            self.actor.decode()?,
            kind,
        ))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedModelChange {
    before: Provider,
    after: Provider,
    context: String,
    actor: Actor,
    before_configuration: SavedModelConfiguration,
    requested_configuration: SavedModelConfiguration,
    configuration: SavedModelConfiguration,
    timing: Option<String>,
}
impl From<&ModelChangeRecord> for SavedModelChange {
    fn from(record: &ModelChangeRecord) -> Self {
        let change = record.change();
        Self {
            before: Provider {
                name: change.before().name().into(),
                model_id: change.before().model_id().into(),
                context: change.before().context().into(),
            },
            after: Provider {
                name: change.after().name().into(),
                model_id: change.after().model_id().into(),
                context: change.after().context().into(),
            },
            context: change.context().into(),
            actor: record.actor().into(),
            before_configuration: change.prior_configuration().into(),
            requested_configuration: change.requested_configuration().into(),
            configuration: record.configuration().into(),
            timing: record.timing().map(|timing| {
                match timing {
                    ModelChangeTiming::SubsequentResponses => "subsequentResponses",
                    ModelChangeTiming::NextTurn => "nextTurn",
                }
                .into()
            }),
        }
    }
}
impl SavedModelChange {
    pub(super) fn decode(self) -> Result<ModelChangeRecord, StorageError> {
        Ok(ModelChangeRecord::new(
            ModelChange::restore(
                self.before.decode()?,
                self.after.decode()?,
                self.context,
                self.before_configuration.decode()?,
                self.requested_configuration.decode()?,
            )
            .map_err(corrupt)?,
            self.actor.decode()?,
            self.configuration.decode()?,
            self.timing
                .map(|timing| match timing.as_str() {
                    "subsequentResponses" => Ok(ModelChangeTiming::SubsequentResponses),
                    "nextTurn" => Ok(ModelChangeTiming::NextTurn),
                    _ => Err(corrupt("unknown model application boundary")),
                })
                .transpose()?,
        ))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedModelConfiguration {
    approval_mode: Option<String>,
    effort_level: Option<String>,
}
impl From<&ModelConfiguration> for SavedModelConfiguration {
    fn from(value: &ModelConfiguration) -> Self {
        Self {
            approval_mode: value.approval_mode().map(|mode| mode.as_str().into()),
            effort_level: value.effort_level().map(|level| level.as_str().into()),
        }
    }
}
impl SavedModelConfiguration {
    pub(super) fn decode(self) -> Result<ModelConfiguration, StorageError> {
        let mode = self
            .approval_mode
            .map(|mode| match mode.as_str() {
                "ask" => Ok(ApprovalMode::Ask),
                "auto" => Ok(ApprovalMode::Auto),
                "full" => Ok(ApprovalMode::Full),
                _ => Err(corrupt("unknown model approval preset")),
            })
            .transpose()?;
        let effort = self
            .effort_level
            .map(EffortLevel::new)
            .transpose()
            .map_err(corrupt)?;
        Ok(ModelConfiguration::new(mode, effort))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum WireChange<E = Event> {
    ModelChanged(SavedModelChange),
    ConfigurationChanged(SavedConfigurationChange),
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
    DeclinePrepared {
        execution_id: String,
        decision: super::records::SavedDecline,
    },
    DeclineAcknowledged {
        execution_id: String,
    },
    CancellationPrepared {
        execution_id: String,
        decision: super::records::SavedCancellation,
    },
    CancellationAcknowledged {
        execution_id: String,
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
    ProviderIdentityChanged {
        before: Provider,
        after: Provider,
    },
    Lease(WireLease),
    Artifact(WireArtifact),
    Note(WireNote),
    NoteRetired(WireNoteRetired),
    Handoff(WireHandoff),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBatch<C = WireChange> {
    /// Required. A batch with no `schemaVersion` is refused in preflight as
    /// another version.
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
            SessionChange::ModelChanged(record) => Self::ModelChanged(record.into()),
            SessionChange::ConfigurationChanged(record) => {
                Self::ConfigurationChanged(record.into())
            }
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
            SessionChange::DeclinePrepared {
                execution_id,
                decision,
            } => Self::DeclinePrepared {
                execution_id: execution_id.as_str().into(),
                decision: decision.into(),
            },
            SessionChange::DeclineAcknowledged { execution_id } => Self::DeclineAcknowledged {
                execution_id: execution_id.as_str().into(),
            },
            SessionChange::CancellationPrepared {
                execution_id,
                decision,
            } => Self::CancellationPrepared {
                execution_id: execution_id.as_str().into(),
                decision: decision.into(),
            },
            SessionChange::CancellationAcknowledged { execution_id } => {
                Self::CancellationAcknowledged {
                    execution_id: execution_id.as_str().into(),
                }
            }
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
            SessionChange::ProviderIdentityChanged { before, after } => {
                Self::ProviderIdentityChanged {
                    before: before.into(),
                    after: after.into(),
                }
            }
            SessionChange::Lease(record) => Self::Lease(record.into()),
            SessionChange::Artifact(record) => Self::Artifact(record.into()),
            SessionChange::Note(record) => Self::Note(record.into()),
            SessionChange::NoteRetired(record) => Self::NoteRetired(record.into()),
            SessionChange::Handoff(record) => Self::Handoff(record.into()),
        }
    }
}

impl TryFrom<WireChange> for SessionChange {
    type Error = StorageError;

    fn try_from(value: WireChange) -> Result<Self, Self::Error> {
        Ok(match value {
            WireChange::ModelChanged(record) => Self::ModelChanged(record.decode()?),
            WireChange::ConfigurationChanged(record) => {
                Self::ConfigurationChanged(record.decode()?)
            }
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
            WireChange::DeclinePrepared {
                execution_id,
                decision,
            } => Self::DeclinePrepared {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                decision: decision.decode()?,
            },
            WireChange::DeclineAcknowledged { execution_id } => Self::DeclineAcknowledged {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
            },
            WireChange::CancellationPrepared {
                execution_id,
                decision,
            } => Self::CancellationPrepared {
                execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                decision: decision.decode()?,
            },
            WireChange::CancellationAcknowledged { execution_id } => {
                Self::CancellationAcknowledged {
                    execution_id: ExecutionId::new(execution_id).map_err(corrupt)?,
                }
            }
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
            WireChange::ProviderIdentityChanged { before, after } => {
                Self::ProviderIdentityChanged {
                    before: before.decode()?,
                    after: after.decode()?,
                }
            }
            WireChange::Lease(record) => Self::Lease(record.decode()?),
            WireChange::Artifact(record) => Self::Artifact(record.decode()?),
            WireChange::Note(record) => Self::Note(record.decode()?),
            WireChange::NoteRetired(record) => Self::NoteRetired(record.decode()?),
            WireChange::Handoff(record) => Self::Handoff(record.decode()?),
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
            Ok(SessionChange::ProviderObservation(event.decode(
                CancellationContext::AtFact(provider_context),
                &execution_id,
            )?))
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
    // Preflight already classified `schemaVersion`. The field stays so a body
    // without it cannot deserialize.
    let _ = batch.schema_version;
    let mut context = context.clone();
    let changes = batch
        .changes
        .into_iter()
        .map(|wire| decode_wire_change(wire, &mut context))
        .collect::<Result<Vec<_>, _>>()?;
    SessionSaveUnit::check_changes(&changes)?;
    Ok(changes)
}

/// One change inside a batch the preflight accepted.
pub(crate) enum ReadChange {
    /// The change decoded.
    Decoded(Box<SessionChange>),
    /// The change did not decode. `lease` is set when the object is a lease.
    Failed { lease: bool },
}

/// A save unit read one change at a time. [`BatchRead::Whole`] means the unit
/// itself cannot be read, so none of its changes apply.
pub(crate) enum BatchRead {
    /// Preflight failed, or the body is not a batch of changes. `lease` is
    /// set when that failure is a lease object of a current record.
    Whole { lease: bool },
    /// Preflight passed. A failed change hides the rest of the unit.
    Changes(Vec<ReadChange>),
}

/// Read a save unit without refusing the chat. A version or shape this build
/// does not read is [`BatchRead::Whole`]. A single change that does not decode
/// is [`ReadChange::Failed`] and the changes after it are not returned.
pub(crate) fn read_batch(bytes: &[u8], context: &ProviderContext) -> BatchRead {
    let walk = super::decode::walk_semantic_batch(bytes);
    if walk.result.is_err() {
        return BatchRead::Whole {
            lease: walk.lease_failed,
        };
    }
    // Preflight permits a repeated field, and `serde_json::Value` then keeps
    // the last one. A visitor that refuses the repeated key runs before any
    // change is converted, so the unit is one gap and the last definition
    // does not apply. A repeated key inside the lease object is still that
    // lease failing to decode.
    let value = match json_without_duplicate_keys(bytes) {
        Ok(value) => value,
        Err(lease) => return BatchRead::Whole { lease },
    };
    let Some(changes) = value.get("changes").and_then(|changes| changes.as_array()) else {
        return BatchRead::Whole { lease: false };
    };
    if changes.is_empty() {
        return BatchRead::Whole { lease: false };
    }
    let mut context = context.clone();
    let mut read = Vec::with_capacity(changes.len());
    for change in changes {
        let lease_object = change
            .as_object()
            .is_some_and(|object| object.contains_key("Lease"));
        let Ok(wire) = serde_json::from_value::<WireChange>(change.clone()) else {
            read.push(ReadChange::Failed {
                lease: lease_object,
            });
            break;
        };
        let lease = lease_object || matches!(wire, WireChange::Lease(_));
        match decode_wire_change(wire, &mut context) {
            Ok(decoded) => read.push(ReadChange::Decoded(Box::new(decoded))),
            Err(_) => {
                read.push(ReadChange::Failed { lease });
                break;
            }
        }
    }
    BatchRead::Changes(read)
}

/// JSON objects keep every key. A repeated name, including one written with a
/// different escape, is refused before a typed conversion can collapse it.
fn json_without_duplicate_keys(bytes: &[u8]) -> Result<Value, bool> {
    let lease = std::cell::Cell::new(false);
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueJson {
        in_lease: false,
        lease: &lease,
    }
    .deserialize(&mut deserializer);
    match value {
        Ok(value) => deserializer.end().map(|()| value).map_err(|_| lease.get()),
        Err(_) => Err(lease.get()),
    }
}

struct UniqueJson<'a> {
    in_lease: bool,
    lease: &'a std::cell::Cell<bool>,
}

impl<'de> DeserializeSeed<'de> for UniqueJson<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(UniqueJsonVisitor {
            in_lease: self.in_lease,
            lease: self.lease,
        })
    }
}

struct UniqueJsonVisitor<'a> {
    in_lease: bool,
    lease: &'a std::cell::Cell<bool>,
}

impl<'de> Visitor<'de> for UniqueJsonVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object keys")
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }

    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(UniqueJson {
            in_lease: self.in_lease,
            lease: self.lease,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                if self.in_lease || key == "Lease" {
                    self.lease.set(true);
                }
                return Err(A::Error::custom("duplicate JSON object key"));
            }
            let in_lease = self.in_lease || key == "Lease";
            values.insert(
                key,
                map.next_value_seed(UniqueJson {
                    in_lease,
                    lease: self.lease,
                })?,
            );
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
            permissions::{ActionContext, ApprovalRuleReference, DeclinedReview},
            providers::ProviderIdentity,
            sessions::{InvocationRecord, SubmissionAcknowledgement},
        },
        domain::agent_execution::{
            executions::{ExecutionOutcome, GrantInitiator, Initiation, MessageChunk, MessageId},
            permissions::{
                RefusalOutcome, ReviewDecline, ReviewDeclineId, ReviewDeclineReason,
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
            decline: None,
            cancellation_delivery: None,
            resumed_from: None,
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: id,
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            handoff: None,
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
            decline: None,
            cancellation_delivery: None,
            resumed_from: None,
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: id,
                user_message: UserMessage::new(None, vec![image], Vec::new(), Vec::new()).unwrap(),
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            handoff: None,
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
    fn stored_file_input_roundtrips_without_a_placement_path_and_rejects_corrupt_names() {
        use crate::domain::agent_execution::prompts::UploadedFileReference;
        use crate::domain::common::value_objects::MediaType;
        let file = UploadedFileReference::new(
            Sha256Digest::from_bytes([4; 32]),
            MediaType::parse("application/pdf").unwrap(),
            12,
            "report.pdf".into(),
        )
        .unwrap();
        let record = InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Queued,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("file-turn").unwrap(),
                user_message: UserMessage::new(None, vec![], vec![], vec![file]).unwrap(),
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            initiation: Initiation::owner(),
            notes: vec![],
            handoff: None,
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: vec![],
            scheduling: vec![],
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        let bytes = encode_one(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
        assert!(
            matches!(decode_one(&bytes, &ProviderContext::Absent).unwrap(), SessionChange::InputAccepted(next) if *next == record)
        );
        let mut stopped = record.clone();
        stopped.submission = SubmissionMode::Immediate;
        stopped.cancellation = Some(
            crate::application::agent_execution::sessions::InvocationCancellationEvent {
                cause: crate::domain::agent_execution::executions::SchedulingCause::TurnStopped,
                actor: None,
            },
        );
        stopped.local_outcome = Some(ExecutionOutcome::Cancelled);
        stopped.result = Some(Ok(ExecutionOutcome::Cancelled));
        let cancelled =
            encode_one(&SessionChange::InputAccepted(Box::new(stopped.clone()))).unwrap();
        assert!(
            matches!(decode_one(&cancelled, &ProviderContext::Absent).unwrap(), SessionChange::InputAccepted(next) if *next == stopped)
        );
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let upload = &original["changes"][0]["InputAccepted"]["metadata"]["user_uploads"][0];
        assert_eq!(upload["name"], "report.pdf");
        assert!(upload.get("path").is_none());
        for (field, invalid) in [
            ("name", serde_json::json!("../escape")),
            ("size", serde_json::json!(0)),
            ("digest", serde_json::json!("sha256:00")),
            ("media_type", serde_json::json!("PDF")),
        ] {
            let mut changed = original.clone();
            changed["changes"][0]["InputAccepted"]["metadata"]["user_uploads"][0][field] = invalid;
            assert!(
                matches!(
                    decode_one(
                        &serde_json::to_vec(&changed).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{field}"
            );
        }
    }

    #[test]
    fn semantic_admission_restores_who_started_the_turn() {
        let record = |initiation| InvocationRecord {
            decline: None,
            cancellation_delivery: None,
            resumed_from: None,
            target_event_offset: None,
            submission: SubmissionMode::Queued,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("one").unwrap(),
                user_message: UserMessage::text_only(PromptText::new("hello").unwrap()),
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            initiation,
            notes: Vec::new(),
            handoff: None,
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        };
        let saved = |record: &InvocationRecord| {
            let bytes =
                encode_one(&SessionChange::InputAccepted(Box::new(record.clone()))).unwrap();
            assert!(matches!(
                decode_one(&bytes, &ProviderContext::Absent).unwrap(),
                SessionChange::InputAccepted(next) if *next == *record
            ));
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };
        let owner = saved(&record(Initiation::owner()));
        assert_eq!(
            owner["changes"][0]["InputAccepted"]["metadata"]["initiation"],
            serde_json::json!({"initiator": "Owner", "tools": "NotChecked"})
        );
        let grant = saved(&record(Initiation::grant(
            GrantInitiator::new("grant-1", 4).unwrap(),
        )));
        assert_eq!(
            grant["changes"][0]["InputAccepted"]["metadata"]["initiation"],
            serde_json::json!({
                "initiator": {"Grant": {"grant": "grant-1", "policy_revision": 4}},
                "tools": "NotChecked"
            })
        );
        let checked = saved(&record(Initiation::checked_grant(
            GrantInitiator::new("grant-1", 4).unwrap(),
        )));
        assert_eq!(
            checked["changes"][0]["InputAccepted"]["metadata"]["initiation"],
            serde_json::json!({
                "initiator": {"Grant": {"grant": "grant-1", "policy_revision": 4}},
                "tools": "Checked"
            })
        );
        let grant = |grant: serde_json::Value| {
            serde_json::json!({
                "initiator": {"Grant": {"grant": grant, "policy_revision": 4}},
                "tools": "NotChecked"
            })
        };
        let invalid = [
            // The domain refuses blank grant text.
            Some(grant(" ".into())),
            // The preflight refuses grant text over its bound before it is built.
            Some(grant(
                "x".repeat(GrantInitiator::MAX_GRANT_BYTES + 1).into(),
            )),
            Some(serde_json::json!({"initiator": "Guest", "tools": "NotChecked"})),
            // Only a grant's turn is checked.
            Some(serde_json::json!({"initiator": "Owner", "tools": "Checked"})),
            Some(serde_json::json!({"initiator": "Owner", "tools": "Unchecked"})),
            Some(serde_json::json!({"initiator": "Owner"})),
            Some(serde_json::json!({"initiator": "Owner", "tools": "NotChecked", "by": 1})),
            // A grant's role is the host's record of the grant, never a copy
            // on the turn: a turn that carries one is not of this shape.
            Some(serde_json::json!({
                "initiator": {"Grant": {"grant": "g", "role": "Drive", "policy_revision": 4}},
                "tools": "NotChecked"
            })),
            Some(serde_json::json!({
                "initiator": {"Grant": {"grant": "g", "policy_revision": -1}},
                "tools": "NotChecked"
            })),
            Some(serde_json::json!({
                "initiator": {
                    "Grant": {"grant": "g", "policy_revision": 1, "more": 1}
                },
                "tools": "NotChecked"
            })),
            // A record without the field is not of this shape.
            None,
        ];
        for replacement in invalid {
            let mut changed = owner.clone();
            let metadata = changed["changes"][0]["InputAccepted"]["metadata"]
                .as_object_mut()
                .unwrap();
            match &replacement {
                Some(value) => {
                    metadata.insert("initiation".into(), value.clone());
                }
                None => {
                    metadata.remove("initiation");
                }
            }
            assert!(
                matches!(
                    decode_one(
                        &serde_json::to_vec(&changed).unwrap(),
                        &ProviderContext::Absent
                    ),
                    Err(StorageError::Corrupt(_))
                ),
                "{replacement:?}"
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
            decline: None,
            cancellation_delivery: None,
            resumed_from: None,
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("one").unwrap(),
                user_message: message,
                estimated_input_tokens: 7,
                reserved_output_tokens: 8,
            },
            actor: ActionContext::new("user", "phone", "send").unwrap(),
            initiation: Initiation::owner(),
            notes: Vec::new(),
            handoff: None,
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
        for field in ["user_app", "user_internal", "user_app_model_context"] {
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
        let mut ambiguous = valid.clone();
        ambiguous["changes"][0]["InputAccepted"]["metadata"]["user_internal"] = true.into();
        assert!(matches!(
            decode_one(
                &serde_json::to_vec(&ambiguous).unwrap(),
                &ProviderContext::Absent
            ),
            Err(StorageError::Corrupt(_))
        ));
        let mut internal = record.clone();
        internal.request.user_message =
            UserMessage::text_only(PromptText::new("actual task output").unwrap())
                .sent_by(MessageSender::Internal);
        let bytes = encode_one(&SessionChange::InputAccepted(Box::new(internal.clone()))).unwrap();
        assert!(
            matches!(decode_one(&bytes, &ProviderContext::Absent).unwrap(), SessionChange::InputAccepted(next) if *next == internal)
        );
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
            let selected = DeclinedReview::selected(
                ReviewDeclineId::new(decline_id).unwrap(),
                ReviewDecline::new(
                    Some("Read"),
                    ReviewDeclineReason::ToolNotReviewable,
                    RefusalOutcome::Continue,
                ),
                None,
            )
            .unwrap();
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

    fn stopping_rule() -> ApprovalRuleReference {
        ApprovalRuleReference::new(
            "refuse-shell",
            "1",
            ActionContext::new("host", "policy", "grant").unwrap(),
        )
        .unwrap()
    }

    fn declined_event(review: DeclinedReview) -> ExecutionEvent {
        ExecutionEvent::new(
            ExecutionId::new("one").unwrap(),
            ExecutionUpdate::ReviewDeclined(review),
        )
    }

    #[test]
    fn a_stopping_rule_round_trips_and_a_disagreement_is_corrupt() {
        let stopped = DeclinedReview::selected(
            ReviewDeclineId::new("4").unwrap(),
            ReviewDecline::new(
                Some("Bash"),
                ReviewDeclineReason::ToolNotReviewable,
                RefusalOutcome::EndTurn,
            ),
            Some(stopping_rule()),
        )
        .unwrap();
        let event = declined_event(stopped);
        let bytes = encode_one(&SessionChange::ProviderObservation(event.clone())).unwrap();
        let context = ProviderContext::Recorded(ExecutionSessionId::new("provider").unwrap());
        let SessionChange::ProviderObservation(restored) = decode_one(&bytes, &context).unwrap()
        else {
            panic!("decoded fact remains an observation");
        };
        let ExecutionUpdate::ReviewDeclined(review) = restored.update() else {
            panic!("restored event is the declined review");
        };
        let rule = review.rule().expect("a restored stop names its rule");
        assert_eq!(rule.rule_id(), "refuse-shell");
        assert_eq!(rule.revision(), "1");
        assert_eq!(rule.granted_by().principal_id(), "host");
        assert_eq!(rule.granted_by().surface_id(), "policy");
        assert_eq!(rule.granted_by().request_id(), "grant");
        assert_eq!(restored, event);

        let mut stripped: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        stripped["changes"][0]["ProviderObservation"]["update"]["ReviewDeclined"]
            .as_object_mut()
            .unwrap()
            .remove("rule");
        assert!(matches!(
            decode_one(&serde_json::to_vec(&stripped).unwrap(), &context),
            Err(StorageError::Corrupt(_))
        ));

        let continued = declined_event(
            DeclinedReview::selected(
                ReviewDeclineId::new("5").unwrap(),
                ReviewDecline::new(
                    Some("Read"),
                    ReviewDeclineReason::UnreadableRequest,
                    RefusalOutcome::Continue,
                ),
                None,
            )
            .unwrap(),
        );
        let continued_bytes = encode_one(&SessionChange::ProviderObservation(continued)).unwrap();
        let mut attached: serde_json::Value = serde_json::from_slice(&continued_bytes).unwrap();
        attached["changes"][0]["ProviderObservation"]["update"]["ReviewDeclined"]["rule"] = serde_json::json!({
            "rule_id": "refuse-shell",
            "revision": "1",
            "granted_by": {
                "principal_id": "host",
                "surface_id": "policy",
                "request_id": "grant"
            }
        });
        assert!(matches!(
            decode_one(&serde_json::to_vec(&attached).unwrap(), &context),
            Err(StorageError::Corrupt(_))
        ));

        let mut oversized = attached;
        oversized["changes"][0]["ProviderObservation"]["update"]["ReviewDeclined"]["outcome"] =
            "EndTurn".into();
        oversized["changes"][0]["ProviderObservation"]["update"]["ReviewDeclined"]["rule"]
            ["rule_id"] = "r".repeat(257).into();
        assert!(matches!(
            decode_one(&serde_json::to_vec(&oversized).unwrap(), &context),
            Err(StorageError::Corrupt(_))
        ));
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
    fn a_provider_identity_change_round_trips() {
        let change = SessionChange::ProviderIdentityChanged {
            before: ProviderIdentity::new("fixture", "model", "workspace:one").unwrap(),
            after: ProviderIdentity::new("fixture", "model", "workspace:two").unwrap(),
        };
        let bytes = encode_batch(std::slice::from_ref(&change)).unwrap();
        assert_eq!(
            decode_batch(&bytes, &ProviderContext::Absent).unwrap(),
            [change]
        );
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

    #[test]
    fn model_and_later_settings_round_trip_with_exact_actor_and_before_state() {
        use crate::application::agent_execution::providers::{
            ApprovalMode, ModelChange, ModelChangeRecord, ModelChangeTiming, ModelConfiguration,
            SessionConfigurationKind, SessionConfigurationRecord,
        };
        let before = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
        let after =
            ProviderIdentity::new("fixture", "another", "workspace-with-other-limits").unwrap();
        let actor = ActionContext::new("person", "window", "switch-model").unwrap();
        let configuration = ModelConfiguration::new(Some(ApprovalMode::Ask), None);
        let model = ModelChangeRecord::new(
            ModelChange::restore(
                before.clone(),
                after.clone(),
                "same-workspace".into(),
                ModelConfiguration::new(Some(ApprovalMode::Ask), None),
                ModelConfiguration::new(Some(ApprovalMode::Ask), None),
            )
            .unwrap(),
            actor.clone(),
            configuration.clone(),
            Some(ModelChangeTiming::NextTurn),
        );
        let changed = SessionChange::ModelChanged(model.clone());
        assert_eq!(
            decode_one(&encode_one(&changed).unwrap(), &ProviderContext::Absent).unwrap(),
            changed
        );
        let initial = crate::application::agent_execution::sessions::records::fold_changes(
            None,
            &[opened_change()],
        )
        .unwrap();
        let selected = crate::application::agent_execution::sessions::records::fold_changes(
            Some(&initial),
            std::slice::from_ref(&changed),
        )
        .unwrap();
        assert_eq!(selected.provider, after);
        assert_eq!(selected.model_change.as_ref(), Some(&model));
        let mode_actor = ActionContext::new("person", "window", "change-mode").unwrap();
        let mode = SessionConfigurationRecord::new(
            Some(configuration),
            ModelConfiguration::new(Some(ApprovalMode::Auto), None),
            mode_actor.clone(),
            SessionConfigurationKind::ApprovalMode,
        );
        let changed_mode = SessionChange::ConfigurationChanged(mode.clone());
        assert_eq!(
            decode_one(
                &encode_one(&changed_mode).unwrap(),
                &ProviderContext::Absent
            )
            .unwrap(),
            changed_mode
        );
        let latest = crate::application::agent_execution::sessions::records::fold_changes(
            Some(&selected),
            std::slice::from_ref(&changed_mode),
        )
        .unwrap();
        assert_eq!(latest.model_change.as_ref(), Some(&model));
        assert_eq!(latest.current_configuration.as_ref(), Some(&mode));
        assert_eq!(
            latest.current_configuration.as_ref().unwrap().actor(),
            &mode_actor
        );
        assert!(
            crate::application::agent_execution::sessions::records::fold_changes(
                Some(&latest),
                &[changed_mode]
            )
            .is_err()
        );
        assert!(
            crate::application::agent_execution::sessions::records::fold_changes(
                Some(&selected),
                &[changed]
            )
            .is_err()
        );
        let bytes = serde_json::to_vec(&super::super::checkpoint::SnapshotRef(&latest)).unwrap();
        let saved: super::super::checkpoint::Snapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved.decode().unwrap(), latest);
    }

    #[test]
    fn configuration_relationships_match_semantic_fold_and_checkpoint_restore() {
        use crate::application::agent_execution::providers::{
            ApprovalMode, ModelChange, ModelChangeRecord, ModelConfiguration,
            SessionConfigurationKind, SessionConfigurationRecord,
        };
        use crate::application::agent_execution::sessions::records::fold_changes;
        use crate::domain::model_metadata::value_objects::EffortLevel;
        let configuration = |mode, effort: &str| {
            ModelConfiguration::new(Some(mode), Some(EffortLevel::new(effort.into()).unwrap()))
        };
        let ask_high = configuration(ApprovalMode::Ask, "high");
        let ask_low = configuration(ApprovalMode::Ask, "low");
        let auto_high = configuration(ApprovalMode::Auto, "high");
        let actor = ActionContext::new("person", "window", "model").unwrap();
        let other = ActionContext::new("person", "window", "later-setting").unwrap();
        let initial = fold_changes(None, &[opened_change()]).unwrap();
        let model = ModelChangeRecord::new(
            ModelChange::restore(
                initial.provider.clone(),
                ProviderIdentity::new("fixture", "other", "context").unwrap(),
                "workspace".into(),
                ask_high.clone(),
                ask_high.clone(),
            )
            .unwrap(),
            actor.clone(),
            ask_high.clone(),
            None,
        );
        let selected = fold_changes(
            Some(&initial),
            &[SessionChange::ModelChanged(model.clone())],
        )
        .unwrap();
        let cases = [
            (
                "approval changes effort",
                SessionConfigurationKind::ApprovalMode,
                Some(ask_high.clone()),
                ask_low.clone(),
                other.clone(),
                false,
            ),
            (
                "effort changes approval",
                SessionConfigurationKind::EffortLevel,
                Some(ask_high.clone()),
                auto_high.clone(),
                other.clone(),
                false,
            ),
            (
                "missing prior",
                SessionConfigurationKind::ApprovalMode,
                None,
                auto_high.clone(),
                other.clone(),
                false,
            ),
            (
                "model actor differs",
                SessionConfigurationKind::Model,
                Some(ask_high.clone()),
                ask_high.clone(),
                other.clone(),
                false,
            ),
            (
                "model prior differs",
                SessionConfigurationKind::Model,
                Some(ask_low.clone()),
                ask_high.clone(),
                actor.clone(),
                false,
            ),
            (
                "model after differs",
                SessionConfigurationKind::Model,
                Some(ask_high.clone()),
                ask_low.clone(),
                actor.clone(),
                false,
            ),
            (
                "valid model",
                SessionConfigurationKind::Model,
                Some(ask_high.clone()),
                ask_high.clone(),
                actor.clone(),
                true,
            ),
            (
                "valid later approval",
                SessionConfigurationKind::ApprovalMode,
                Some(ask_high.clone()),
                auto_high,
                other.clone(),
                true,
            ),
            (
                "valid later effort",
                SessionConfigurationKind::EffortLevel,
                Some(ask_high),
                ask_low,
                other,
                true,
            ),
        ];
        for (name, kind, before, after, actor, valid) in cases {
            let record = SessionConfigurationRecord::new(before, after, actor, kind);
            let mut snapshot = selected.clone();
            snapshot.current_configuration = Some(record.clone());
            let later = SessionConfigurationRecord::new(
                Some(record.after().clone()),
                record.after().clone(),
                record.actor().clone(),
                SessionConfigurationKind::ApprovalMode,
            );
            assert_eq!(
                fold_changes(
                    Some(&snapshot),
                    &[SessionChange::ConfigurationChanged(later)]
                )
                .is_ok(),
                valid,
                "malformed prior cannot be hidden by later control: {name}"
            );
            let bytes =
                serde_json::to_vec(&super::super::checkpoint::SnapshotRef(&snapshot)).unwrap();
            let checkpoint: super::super::checkpoint::Snapshot =
                serde_json::from_slice(&bytes).unwrap();
            assert_eq!(checkpoint.decode().is_ok(), valid, "checkpoint: {name}");
            if kind != SessionConfigurationKind::Model {
                assert_eq!(
                    fold_changes(
                        Some(&selected),
                        &[SessionChange::ConfigurationChanged(record)]
                    )
                    .is_ok(),
                    valid,
                    "semantic fold: {name}"
                );
            }
        }
        for missing_model in [true, false] {
            let mut snapshot = selected.clone();
            if missing_model {
                snapshot.model_change = None;
            } else {
                snapshot.current_configuration = None;
            }
            let bytes =
                serde_json::to_vec(&super::super::checkpoint::SnapshotRef(&snapshot)).unwrap();
            let checkpoint: super::super::checkpoint::Snapshot =
                serde_json::from_slice(&bytes).unwrap();
            assert!(
                checkpoint.decode().is_err(),
                "missing model/current relationship: {missing_model}"
            );
        }
    }

    fn opened_change() -> SessionChange {
        SessionChange::Opened {
            id: SessionId::new("conversation").unwrap(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        }
    }

    #[test]
    fn a_saved_batch_writes_the_current_schema_version() {
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
    fn a_turn_carrying_its_grants_role_is_corrupt() {
        let saved = crate::infrastructure::session_storage::ROLE_COPYING_SESSION_BATCH;
        assert!(matches!(
            decode_batch(saved, &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
        // The role is the batch's only defect: without it, the same bytes
        // are a current record.
        let current =
            String::from_utf8(saved.to_vec())
                .unwrap()
                .replacen(r#""role":"Drive","#, "", 1);
        let changes = decode_batch(current.as_bytes(), &ProviderContext::Absent).unwrap();
        assert_eq!(changes[0], opened_change());
        assert!(matches!(
            &changes[1],
            SessionChange::InputAccepted(record)
                if record.initiation
                    == Initiation::grant(GrantInitiator::new("receiver", 1).unwrap())
        ));
    }

    #[test]
    fn an_unmarked_or_other_version_is_not_read() {
        // Bytes main's encoder writes for one Opened and one InputAccepted.
        assert_eq!(
            decode_batch(
                crate::infrastructure::session_storage::UNMARKED_SESSION_BATCH,
                &ProviderContext::Absent
            ),
            Err(StorageError::AnotherVersion { found: None })
        );
        let bytes = encode_one(&opened_change()).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for found in [0, 1, 2, StorageError::SCHEMA_VERSION + 1] {
            let mut other = saved.clone();
            other["schemaVersion"] = serde_json::json!(found);
            other["notInThisShape"] = serde_json::json!(true);
            assert_eq!(
                decode_batch(
                    &serde_json::to_vec(&other).unwrap(),
                    &ProviderContext::Absent
                ),
                Err(StorageError::AnotherVersion { found: Some(found) })
            );
        }
        let current = StorageError::SCHEMA_VERSION;
        let later = current + 1;
        let doubled = String::from_utf8(bytes.clone()).unwrap().replacen(
            &format!("\"schemaVersion\":{current}"),
            &format!("\"schemaVersion\":{current},\"schemaVersion\":{later}"),
            1,
        );
        assert_eq!(
            decode_batch(doubled.as_bytes(), &ProviderContext::Absent),
            Err(StorageError::AnotherVersion { found: Some(later) })
        );
        // The unsupported marker ends the walk, so a later current marker is
        // not read.
        let stopped = String::from_utf8(bytes).unwrap().replacen(
            &format!("\"schemaVersion\":{current}"),
            &format!("\"schemaVersion\":{later},\"schemaVersion\":{current}"),
            1,
        );
        assert_eq!(
            decode_batch(stopped.as_bytes(), &ProviderContext::Absent),
            Err(StorageError::AnotherVersion { found: Some(later) })
        );
    }

    #[test]
    fn a_duplicate_json_field_is_not_read_as_its_last_value() {
        let kept = encode_batch(&[opened_change()]).unwrap();
        let other = SessionChange::Opened {
            id: SessionId::new("other-conversation").unwrap(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let last = encode_batch(&[other]).unwrap();
        let last_text = String::from_utf8(last).unwrap();
        let changes_at = last_text.find("\"changes\":").unwrap();
        let repeated_changes = {
            let mut text = String::from_utf8(kept.clone()).unwrap();
            text.pop();
            text.push(',');
            text.push_str(&last_text[changes_at..last_text.len() - 1]);
            text.push('}');
            text.into_bytes()
        };
        let collapsed: serde_json::Value = serde_json::from_slice(&repeated_changes).unwrap();
        assert_eq!(
            collapsed["changes"][0]["Opened"]["id"],
            "other-conversation"
        );
        assert!(matches!(
            read_batch(&repeated_changes, &ProviderContext::Absent),
            BatchRead::Whole { .. }
        ));

        let nested = String::from_utf8(kept.clone()).unwrap().replacen(
            "\"id\":",
            "\"id\":\"conversation\",\"id\":",
            1,
        );
        let collapsed: serde_json::Value = serde_json::from_slice(nested.as_bytes()).unwrap();
        assert_eq!(collapsed["changes"][0]["Opened"]["id"], "conversation");
        assert!(matches!(
            read_batch(nested.as_bytes(), &ProviderContext::Absent),
            BatchRead::Whole { .. }
        ));

        let escaped = String::from_utf8(kept).unwrap().replacen(
            "\"id\":\"conversation\"",
            "\"id\":\"first\",\"\\u0069d\":\"conversation\"",
            1,
        );
        let collapsed: serde_json::Value = serde_json::from_slice(escaped.as_bytes()).unwrap();
        assert_eq!(collapsed["changes"][0]["Opened"]["id"], "conversation");
        assert!(matches!(
            read_batch(escaped.as_bytes(), &ProviderContext::Absent),
            BatchRead::Whole { .. }
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
            serde_json::json!(true),
            serde_json::json!([]),
            serde_json::json!({}),
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
        let mut nested = saved.clone();
        nested["changes"][0]["Opened"]["schemaVersion"] = serde_json::json!(99);
        assert!(matches!(
            decode_batch(
                &serde_json::to_vec(&nested).unwrap(),
                &ProviderContext::Absent
            ),
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            decode_batch(b"[]", &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        ));
        let mut broken = saved;
        broken["changes"] = serde_json::json!("not a list");
        assert!(matches!(
            decode_batch(
                &serde_json::to_vec(&broken).unwrap(),
                &ProviderContext::Absent
            ),
            Err(StorageError::Corrupt(_))
        ));
    }
}
