//! The original source replacement, independent of its current cursor view.
use super::super::{
    codec::{self, SavedScope, SourceStop},
    cursor, integer, invalid, uuid,
};
use crate::webhooks::application::{OutboxError, SOURCE_RESET_BYTES};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedFact {
    id: String,
    registration: String,
    conversation: String,
    prior_scope: SavedScope,
    prior_position: u64,
    observed_scope: SavedScope,
    stop: SourceStop,
}

pub(in super::super) struct Fact {
    pub id: Uuid,
    pub registration: Uuid,
    pub conversation: ConversationId,
    pub prior_scope: Scope,
    pub prior_position: u64,
    pub observed_scope: Scope,
    pub stop: SourceStop,
}
impl Fact {
    pub fn encode(&self) -> Result<String, OutboxError> {
        self.validate()?;
        let text = serde_json::to_string(&SavedFact {
            id: self.id.to_string(),
            registration: self.registration.to_string(),
            conversation: self.conversation.to_string(),
            prior_scope: SavedScope::of(&self.prior_scope),
            prior_position: self.prior_position,
            observed_scope: SavedScope::of(&self.observed_scope),
            stop: self.stop,
        })
        .map_err(invalid)?;
        if text.len() > SOURCE_RESET_BYTES {
            return Err(OutboxError::Invalid);
        }
        Ok(text)
    }
    pub fn decode(text: &str) -> Result<Self, OutboxError> {
        if text.len() > SOURCE_RESET_BYTES {
            return Err(OutboxError::Invalid);
        }
        let saved: SavedFact = serde_json::from_str(text).map_err(invalid)?;
        let fact = Self {
            id: uuid(&saved.id)?,
            registration: uuid(&saved.registration)?,
            conversation: ConversationId::new(&saved.conversation).map_err(invalid)?,
            prior_scope: saved.prior_scope.checked()?,
            prior_position: saved.prior_position,
            observed_scope: saved.observed_scope.checked()?,
            stop: saved.stop,
        };
        if fact.encode()? != text {
            return Err(OutboxError::Invalid);
        }
        Ok(fact)
    }
    fn validate(&self) -> Result<(), OutboxError> {
        cursor::check_replacement(
            &self.registration.to_string(),
            &self.conversation.to_string(),
            &self.prior_scope,
            &self.observed_scope,
        )?;
        integer(self.prior_position)?;
        integer(self.stop.at_ms)?;
        // Scope encoding retains the owner's byte bound as well as valid IDs.
        codec::scope_text(&self.prior_scope)?;
        codec::scope_text(&self.observed_scope)?;
        Ok(())
    }
}
