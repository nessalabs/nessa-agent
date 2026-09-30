//! Full semantic checkpoint mapping reuses storage field codecs and the shared validator.
use super::{
    queue_order::QueueEvent,
    records::{Event, Metadata, Provider},
    scheduling::SchedulingEvent,
    tools::corrupt,
};
use crate::application::agent_execution::executions::ExecutionEvent;
#[cfg(test)]
use crate::application::agent_execution::{
    executions::{ExecutionRequest, SubmissionMode},
    permissions::ActionContext,
    providers::ProviderIdentity,
    sessions::SubmissionAcknowledgement,
};
#[cfg(test)]
use crate::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{PromptText, UserMessage},
};
use crate::{
    application::agent_execution::sessions::{
        InvocationRecord, InvocationSchedulingEvent, ProviderContext, QueueHistoryRecord,
        SessionSnapshot, StorageError,
    },
    domain::agent_execution::sessions::{ExecutionSessionId, SessionId},
};
use serde::{ser::SerializeSeq, Deserialize, Serialize, Serializer};

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Snapshot {
    value: SnapshotCodec<Vec<Invocation>, Vec<QueueEvent>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotCodec<I, Q> {
    id: String,
    provider: Provider,
    context: Option<String>,
    invocations: I,
    queue_history: Q,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation<S = Vec<SchedulingEvent>, E = Vec<Event>> {
    metadata: Metadata,
    scheduling: S,
    events: E,
}
pub(crate) struct SnapshotRef<'a>(pub &'a SessionSnapshot);
impl Serialize for SnapshotRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = self.0;
        SnapshotCodec {
            id: value.id.as_str().to_owned(),
            provider: Provider {
                name: value.provider.name().into(),
                model_id: value.provider.model_id().into(),
                context: value.provider.context().into(),
            },
            context: value
                .provider_context
                .recorded()
                .map(|id| id.as_str().to_owned()),
            invocations: InvocationList(&value.invocations),
            queue_history: QueueList(&value.queue_history),
        }
        .serialize(serializer)
    }
}
struct InvocationList<'a>(&'a [InvocationRecord]);
impl Serialize for InvocationList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for record in self.0 {
            seq.serialize_element(&Invocation {
                metadata: Metadata::from(record),
                scheduling: SchedulingList(&record.scheduling),
                events: EventList(&record.events),
            })?;
        }
        seq.end()
    }
}
struct SchedulingList<'a>(&'a [InvocationSchedulingEvent]);
impl Serialize for SchedulingList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for event in self.0 {
            seq.serialize_element(&SchedulingEvent::from(event.clone()))?;
        }
        seq.end()
    }
}
struct EventList<'a>(&'a [ExecutionEvent]);
impl Serialize for EventList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for event in self.0 {
            seq.serialize_element(&Event::from(event.clone()))?;
        }
        seq.end()
    }
}
struct QueueList<'a>(&'a [QueueHistoryRecord]);
impl Serialize for QueueList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for event in self.0 {
            seq.serialize_element(&QueueEvent::from(event))?;
        }
        seq.end()
    }
}

impl Snapshot {
    pub(crate) fn decode(self) -> Result<SessionSnapshot, StorageError> {
        let value = self.value;
        let context = value
            .context
            .map(ExecutionSessionId::new)
            .transpose()
            .map_err(corrupt)?;
        let mut invocations = Vec::with_capacity(value.invocations.len());
        for saved in value.invocations {
            let mut record = saved.metadata.decode()?;
            record.scheduling = saved
                .scheduling
                .into_iter()
                .map(SchedulingEvent::decode)
                .collect::<Result<_, _>>()?;
            for event in saved.events {
                let provider = context
                    .as_ref()
                    .ok_or_else(|| corrupt("checkpoint observation has no provider context"))?;
                record
                    .events
                    .push(event.decode(provider, &record.request.execution_id)?);
            }
            invocations.push(record);
        }
        let snapshot = SessionSnapshot {
            id: SessionId::new(value.id).map_err(corrupt)?,
            provider: value.provider.decode()?,
            provider_context: context.map_or(ProviderContext::Absent, ProviderContext::Recorded),
            invocations,
            queue_history: value
                .queue_history
                .into_iter()
                .map(QueueEvent::decode)
                .collect::<Result<_, _>>()?,
        };
        super::validate(&snapshot)?;
        Ok(snapshot)
    }
}

#[cfg(test)]
pub(crate) fn history_fixture(count: usize) -> SessionSnapshot {
    let invocations = (0..count)
        .map(|index| InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: ExecutionId::new(format!("input-{index}")).unwrap(),
                user_message: UserMessage::text_only(
                    PromptText::new("x".repeat(ExecutionRequest::MAX_MESSAGE_BYTES)).unwrap(),
                ),
                estimated_input_tokens: 1,
                reserved_output_tokens: 1,
            },
            actor: ActionContext::new("user", "fixture", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Pending,
            events: Vec::new(),
            scheduling: Vec::new(),
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        })
        .collect();
    SessionSnapshot {
        id: SessionId::new("conversation").unwrap(),
        provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
        provider_context: ProviderContext::Absent,
        invocations,
        queue_history: Vec::new(),
    }
}
