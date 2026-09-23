//! Compact queue membership facts in the same atomic JSONL transaction as lifecycle evidence.
use super::{permissions::Actor, scheduling::Kind, tools::corrupt};
use crate::application::agent_execution::sessions::{QueueHistoryRecord, StorageError};
use crate::domain::agent_execution::executions::{
    ExecutionId, QueueMutation, QueueOrderChange, QueueRemovalCause,
};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueueEvent {
    mutation: Mutation,
    actor: Option<Actor>,
    scheduling_length: Option<usize>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Mutation {
    Admitted {
        id: String,
        kind: Kind,
    },
    Selected {
        id: String,
    },
    Removed {
        id: String,
        cause: RemovalCause,
    },
    Reordered {
        before: Vec<Entry>,
        after: Vec<String>,
    },
    Restored,
}
#[derive(Serialize, Deserialize)]
enum RemovalCause {
    Withdrawn,
    SessionClosed,
    RunnerStopped,
    DispatchFailed,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    kind: Kind,
}
impl From<&QueueHistoryRecord> for QueueEvent {
    fn from(value: &QueueHistoryRecord) -> Self {
        Self {
            mutation: match &value.mutation {
                QueueMutation::Admitted { id, kind } => Mutation::Admitted {
                    id: id.as_str().into(),
                    kind: (*kind).into(),
                },
                QueueMutation::Selected { id } => Mutation::Selected {
                    id: id.as_str().into(),
                },
                QueueMutation::Removed { id, cause } => Mutation::Removed {
                    id: id.as_str().into(),
                    cause: match cause {
                        QueueRemovalCause::Withdrawn => RemovalCause::Withdrawn,
                        QueueRemovalCause::SessionClosed => RemovalCause::SessionClosed,
                        QueueRemovalCause::RunnerStopped => RemovalCause::RunnerStopped,
                        QueueRemovalCause::DispatchFailed => RemovalCause::DispatchFailed,
                    },
                },
                QueueMutation::Reordered(change) => Mutation::Reordered {
                    before: change
                        .before()
                        .iter()
                        .map(|(id, kind)| Entry {
                            id: id.as_str().into(),
                            kind: (*kind).into(),
                        })
                        .collect(),
                    after: change.after().iter().map(|id| id.as_str().into()).collect(),
                },
                QueueMutation::Restored => Mutation::Restored,
            },
            actor: value.actor.as_ref().map(Into::into),
            scheduling_length: value.scheduling_length,
        }
    }
}
impl QueueEvent {
    pub(super) fn decode(self) -> Result<QueueHistoryRecord, StorageError> {
        Ok(QueueHistoryRecord {
            mutation: match self.mutation {
                Mutation::Admitted { id, kind } => QueueMutation::Admitted {
                    id: ExecutionId::new(id).map_err(corrupt)?,
                    kind: kind.into(),
                },
                Mutation::Selected { id } => QueueMutation::Selected {
                    id: ExecutionId::new(id).map_err(corrupt)?,
                },
                Mutation::Removed { id, cause } => QueueMutation::Removed {
                    id: ExecutionId::new(id).map_err(corrupt)?,
                    cause: match cause {
                        RemovalCause::Withdrawn => QueueRemovalCause::Withdrawn,
                        RemovalCause::SessionClosed => QueueRemovalCause::SessionClosed,
                        RemovalCause::RunnerStopped => QueueRemovalCause::RunnerStopped,
                        RemovalCause::DispatchFailed => QueueRemovalCause::DispatchFailed,
                    },
                },
                Mutation::Reordered { before, after } => QueueMutation::Reordered(
                    QueueOrderChange::new(
                        before
                            .into_iter()
                            .map(|entry| {
                                Ok((
                                    ExecutionId::new(entry.id).map_err(corrupt)?,
                                    entry.kind.into(),
                                ))
                            })
                            .collect::<Result<Vec<_>, StorageError>>()?,
                        after
                            .into_iter()
                            .map(|id| ExecutionId::new(id).map_err(corrupt))
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                    .map_err(|error| corrupt(format!("invalid queue order: {error:?}")))?,
                ),
                Mutation::Restored => QueueMutation::Restored,
            },
            actor: self.actor.map(Actor::decode).transpose()?,
            scheduling_length: self.scheduling_length,
        })
    }
}
