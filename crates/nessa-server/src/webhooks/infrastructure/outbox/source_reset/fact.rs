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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selection: Option<Selection>,
}

pub(in super::super) struct Fact {
    pub id: Uuid,
    pub registration: Uuid,
    pub conversation: ConversationId,
    pub prior_scope: Scope,
    pub prior_position: u64,
    pub observed_scope: Scope,
    pub stop: SourceStop,
    pub selection: Option<Selection>,
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
            selection: self.selection.clone(),
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
            selection: saved.selection,
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
        if let Some(selection) = &self.selection {
            selection.validate(self.registration, &self.conversation)?;
            if selection.request == self.id.to_string() {
                return Err(OutboxError::Invalid);
            }
        }
        integer(self.prior_position)?;
        integer(self.stop.at_ms)?;
        // Scope encoding retains the owner's byte bound as well as valid IDs.
        codec::scope_text(&self.prior_scope)?;
        codec::scope_text(&self.observed_scope)?;
        Ok(())
    }
}

/// Immutable owner choice; the original source-change provenance remains in Fact.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in super::super) struct Selection {
    pub request: String,
    pub actor: String,
    pub credential: String,
    pub issued_at_ms: u64,
    pub at_ms: u64,
    pub scope: SavedScope,
}
impl Selection {
    pub fn validate(
        &self,
        registration: Uuid,
        conversation: &ConversationId,
    ) -> Result<(), OutboxError> {
        uuid(&self.request)?;
        nessa_auth::domain::PrincipalId::new(&self.actor).map_err(invalid)?;
        nessa_auth::domain::CredentialId::new(&self.credential).map_err(invalid)?;
        integer(self.issued_at_ms)?;
        integer(self.at_ms)?;
        let scope = self.scope.checked()?;
        cursor::check_identity(&registration.to_string(), &conversation.to_string(), &scope)
    }
}
