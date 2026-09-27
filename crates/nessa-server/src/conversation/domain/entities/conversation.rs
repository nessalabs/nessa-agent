use crate::agents::domain::AgentId;
use crate::conversation::domain::{
    ConversationDeletion, ConversationId, DeletionContradiction, LATEST_TIME_MS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};

/// Why a caller may not act on a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationRefusal {
    /// Not this caller's, or not there at all: the same answer on purpose.
    NotFound,
    /// This caller's, and somebody deleted it. Its identity is not reused.
    Deleted,
}

/// Durable ownership of one conversation, shared by the owner's authenticated surfaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conversation {
    id: ConversationId,
    organization: OrganizationId,
    owner: PrincipalId,
    creator_surface: String,
    creation_action: String,
    creation_requested_at_ms: u64,
    /// `None` for a record naming an agent this build has no adapter for.
    agent: Option<AgentId>,
    deletion: Option<ConversationDeletion>,
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
        Self::restore(
            id,
            organization,
            owner,
            creator_surface,
            creation_action,
            creation_requested_at_ms,
            Some(agent),
        )
    }
    /// A conversation read back from its ownership record. `agent` is `None`
    /// for a record naming an agent this build has no adapter for — one a
    /// newer build added, say. Such a conversation is still its owner's: it is
    /// listed, and it can be deleted; it cannot be opened
    /// (`a_conversation_naming_an_unknown_agent_is_listed_and_deleted_but_not_opened`).
    pub fn restore(
        id: ConversationId,
        organization: OrganizationId,
        owner: PrincipalId,
        creator_surface: String,
        creation_action: String,
        creation_requested_at_ms: u64,
        agent: Option<AgentId>,
    ) -> Result<Self, &'static str> {
        Self::check_creator_context(&creator_surface, &creation_action)?;
        // Listed as `createdAtMs`: never a time a list could not carry
        // (`a_record_created_past_the_latest_time_is_refused`).
        if creation_requested_at_ms > LATEST_TIME_MS {
            return Err("conversation creation time out of range");
        }
        Ok(Self {
            id,
            organization,
            owner,
            creator_surface,
            creation_action,
            creation_requested_at_ms,
            agent,
            deletion: None,
        })
    }
    /// This conversation, deleted by `deletion`. Deletion is for good: no
    /// method of this type takes a deletion away.
    ///
    /// Already deleted, the first decision stands: `deletion` is recorded
    /// over it by [`ConversationDeletion::followed_by`], so a later delete by
    /// another request or surface never becomes the one that decided, and the
    /// same decision only carries it further. The one place that rule is
    /// applied; a repository only persists what this returns
    /// (`the_first_decision_stands_whatever_the_repository_does`).
    ///
    /// # Errors
    /// A tombstone that cannot be this conversation's is refused rather than
    /// attached: one naming anybody but the owner, in its organization, as
    /// deleting it, or dated before the conversation was created
    /// (`a_restored_tombstone_that_contradicts_itself_or_its_conversation_is_refused`).
    pub fn deleted(self, deletion: ConversationDeletion) -> Result<Self, DeletionContradiction> {
        let deletion = match &self.deletion {
            Some(stored) => stored.followed_by(&deletion),
            None => deletion,
        };
        if deletion.organization() != &self.organization || deletion.initiator() != &self.owner {
            return Err(DeletionContradiction::InitiatorNotOwner);
        }
        if deletion.requested_at_ms() < self.creation_requested_at_ms {
            return Err(DeletionContradiction::BeforeCreation);
        }
        Ok(Self {
            deletion: Some(deletion),
            ..self
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
    ///
    /// `None` when the record names an agent this build has no adapter for.
    pub fn agent(&self) -> Option<AgentId> {
        self.agent
    }
    /// The decision that deleted it, if somebody did.
    pub fn deletion(&self) -> Option<&ConversationDeletion> {
        self.deletion.as_ref()
    }
    /// Both organization and principal must agree; knowing an ID grants no access.
    ///
    /// Ownership alone, deleted or not. Whether a caller may act on the
    /// conversation is [`Self::check_access`].
    pub fn allows(&self, organization: &OrganizationId, principal: &PrincipalId) -> bool {
        self.organization == *organization && self.owner == *principal
    }
    /// Whether this caller may act on the conversation: it must be theirs, and
    /// not deleted.
    ///
    /// Ownership is asked first, so only the owner learns that a conversation
    /// was deleted; to anybody else a deleted conversation, like one that is not
    /// theirs, is not found.
    pub fn check_access(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> Result<(), ConversationRefusal> {
        if !self.allows(organization, principal) {
            return Err(ConversationRefusal::NotFound);
        }
        if self.deletion.is_some() {
            return Err(ConversationRefusal::Deleted);
        }
        Ok(())
    }
}
