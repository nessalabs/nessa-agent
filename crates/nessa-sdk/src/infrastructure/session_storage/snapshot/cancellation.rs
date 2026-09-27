//! JSON representation of a local stop, paired with its invocation and lifecycle stage.
use super::{permissions::Actor, scheduling::Cause};
use crate::application::agent_execution::sessions::{InvocationCancellationEvent, StorageError};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cancellation {
    cause: Cause,
    actor: Option<Actor>,
}
impl From<&InvocationCancellationEvent> for Cancellation {
    fn from(value: &InvocationCancellationEvent) -> Self {
        Self {
            cause: value.cause.into(),
            actor: value.actor.as_ref().map(Into::into),
        }
    }
}
impl Cancellation {
    pub(super) fn decode(self) -> Result<InvocationCancellationEvent, StorageError> {
        let value = InvocationCancellationEvent {
            cause: self.cause.into(),
            actor: self.actor.map(Actor::decode).transpose()?,
        };
        value.cancellation()?;
        Ok(value)
    }
}
