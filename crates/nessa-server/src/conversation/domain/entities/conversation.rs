use crate::agents::domain::AgentId;
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::{OrganizationId, PrincipalId};

/// Durable ownership of one conversation, shared by the owner's authenticated surfaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    id: ConversationId,
    organization: OrganizationId,
    owner: PrincipalId,
    creator_surface: String,
    creation_action: String,
    creation_requested_at_ms: u64,
    agent: AgentId,
}
impl Conversation {
    /// Bind an identity to the authenticated creator. Ownership never comes from prompt data.
    pub fn new(
        id: ConversationId,
        organization: OrganizationId,
        owner: PrincipalId,
        creator_surface: String,
        creation_action: String,
        creation_requested_at_ms: u64,
        agent: AgentId,
    ) -> Result<Self, &'static str> {
        Self::check_creator_context(&creator_surface, &creation_action)?;
        Ok(Self {
            id,
            organization,
            owner,
            creator_surface,
            creation_action,
            creation_requested_at_ms,
            agent,
        })
    }
    /// Whether a surface and action are fit to be recorded against a
    /// conversation, before there is one to record them against.
    ///
    /// Held apart from [`Self::new`] because the two questions are asked at
    /// different moments. A creation for a conversation that already exists
    /// builds nothing — it reopens what is on record — and the caller context
    /// still travels with it, into the reopen's own audit record. Leaving the
    /// check inside the constructor meant the same surface and action were
    /// refused for a new conversation and accepted for a reopen, where a
    /// control character then landed unvalidated in a durable
    /// `correlation_id`.
    ///
    /// The authenticated session has already said who this is. What this adds
    /// is that what gets written down is readable: not blank, not unbounded,
    /// and carrying nothing that rewrites a terminal or splits a log line.
    pub fn check_creator_context(
        creator_surface: &str,
        creation_action: &str,
    ) -> Result<(), &'static str> {
        for value in [creator_surface, creation_action] {
            if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err("invalid conversation creator context");
            }
        }
        Ok(())
    }
    pub fn id(&self) -> &ConversationId {
        &self.id
    }
    pub fn organization(&self) -> &OrganizationId {
        &self.organization
    }
    pub fn owner(&self) -> &PrincipalId {
        &self.owner
    }
    pub fn creator_surface(&self) -> &str {
        &self.creator_surface
    }
    pub fn creation_action(&self) -> &str {
        &self.creation_action
    }
    pub fn creation_requested_at_ms(&self) -> u64 {
        self.creation_requested_at_ms
    }
    /// The agent this conversation runs on, fixed when it was created.
    ///
    /// Fixed rather than chosen per prompt: the transcript, the restored
    /// provider session and the permissions already answered all belong to one
    /// agent, and handing them to another is not a switch, it is a different
    /// conversation wearing this one's identity.
    pub fn agent(&self) -> AgentId {
        self.agent
    }
    /// Both organization and principal must agree; knowing an ID grants no access.
    pub fn allows(&self, organization: &OrganizationId, principal: &PrincipalId) -> bool {
        self.organization == *organization && self.owner == *principal
    }
}
