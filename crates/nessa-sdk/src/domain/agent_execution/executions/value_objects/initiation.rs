//! Who started a turn, and whether its tools were checked against a policy.
//! The host decides both before submission; these values only carry the answer
//! so every saved turn says it, and a retry can be compared with the original.
#![deny(missing_docs)]

use crate::domain::agent_execution::ExecutionError;

use super::ExecutionId;

/// Who started a turn: the owner, a grant holder, or an agent in another turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Initiator {
    /// The conversation's owner started the turn.
    Owner,
    /// Someone holding a share grant started the turn.
    Grant(GrantInitiator),
    /// An agent's message, under the source turn the host verified.
    Agent(AgentSource),
}

/// The verified turn whose agent wrote to this conversation. This is origin,
/// not permission to act as the person who started that turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSource {
    conversation: Box<str>,
    execution: ExecutionId,
}
impl AgentSource {
    /// Maximum UTF-8 byte length of a source conversation identity.
    pub const MAX_CONVERSATION_BYTES: usize = 36;

    /// Retain the source conversation and turn identities supplied by the host.
    /// A blank or oversized conversation is refused. The execution uses
    /// [`ExecutionId`]'s value rules.
    pub fn new(
        conversation: impl Into<String>,
        execution: ExecutionId,
    ) -> Result<Self, ExecutionError> {
        Self::checked(conversation.into(), execution)
    }
    // Not generic, so every caller's instantiation of `new` shares one
    // validation body and its failure paths.
    fn checked(conversation: String, execution: ExecutionId) -> Result<Self, ExecutionError> {
        if conversation.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("source conversation"));
        }
        if conversation.len() > Self::MAX_CONVERSATION_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "source conversation",
                max_bytes: Self::MAX_CONVERSATION_BYTES,
            });
        }
        if uuid::Uuid::parse_str(&conversation).map_or(true, |id| id.to_string() != conversation) {
            return Err(ExecutionError::InvalidAgentSource);
        }
        Ok(Self {
            conversation: conversation.into_boxed_str(),
            execution,
        })
    }

    /// The conversation the sending agent was running in.
    pub fn conversation(&self) -> &str {
        &self.conversation
    }
    /// The source turn the agent was running in, or the child turn a report concerns.
    pub fn execution(&self) -> &ExecutionId {
        &self.execution
    }
    /// Variable bytes retained with a message or invocation.
    pub fn payload_bytes(&self) -> usize {
        self.conversation
            .len()
            .saturating_add(self.execution.as_str().len())
    }
}

/// The share grant a turn was started under, as the host admitted it: which
/// grant, and the revision of it the host checked.
///
/// It names the grant and keeps no copy of what the grant allowed. Which
/// grants may start a turn, and with what role, is the host's rule, and the
/// host's record of the grant at that revision is the one place that says
/// what it held. A copy here could only drift from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantInitiator {
    grant: Box<str>,
    policy_revision: u64,
}
impl GrantInitiator {
    /// Maximum UTF-8 byte length of a grant's identity text.
    pub const MAX_GRANT_BYTES: usize = 256;

    /// Retain the grant's exact identity text and the revision of the grant
    /// the host checked. Blank `grant` text
    /// returns [`ExecutionError::EmptyValue`]; text over
    /// [`Self::MAX_GRANT_BYTES`] UTF-8 bytes returns
    /// [`ExecutionError::ValueTooLong`]. This does not check the grant exists.
    pub fn new(grant: impl Into<String>, policy_revision: u64) -> Result<Self, ExecutionError> {
        Self::checked(grant.into(), policy_revision)
    }
    // Not generic, so the checks are one function that every caller's
    // instantiation of `new` shares.
    fn checked(grant: String, policy_revision: u64) -> Result<Self, ExecutionError> {
        if grant.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("share grant"));
        }
        if grant.len() > Self::MAX_GRANT_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "share grant",
                max_bytes: Self::MAX_GRANT_BYTES,
            });
        }
        // Compact storage keeps the retained bytes equal to the text length.
        Ok(Self {
            grant: grant.into_boxed_str(),
            policy_revision,
        })
    }
    /// The grant's exact identity text.
    pub fn grant(&self) -> &str {
        &self.grant
    }
    /// Revision of the grant the host checked: the host's record of the grant
    /// at this revision says what it allowed.
    pub fn policy_revision(&self) -> u64 {
        self.policy_revision
    }
}

/// Whether a turn's tools were checked against a policy before they ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolChecking {
    /// The turn's tools ran without a policy check: the owner's turns.
    NotChecked,
    /// The host checked every tool call the agent asked to make against the
    /// grant's policy at the turn's revision, and admitted the turn only where
    /// it could. What the agent does without asking is not a call it asked
    /// about, and this does not cover it.
    Checked,
}

/// Who started a turn and whether its tools were checked. Saved with the turn
/// and compared on retry, so a retry under a different initiation is a
/// different submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Initiation {
    initiator: Initiator,
    tools: ToolChecking,
}
impl Initiation {
    /// A turn the conversation's owner started; its tools are not checked.
    pub fn owner() -> Self {
        Self {
            initiator: Initiator::Owner,
            tools: ToolChecking::NotChecked,
        }
    }
    /// A turn started under `grant`; its tools are not checked.
    pub fn grant(grant: GrantInitiator) -> Self {
        Self {
            initiator: Initiator::Grant(grant),
            tools: ToolChecking::NotChecked,
        }
    }
    /// A turn started under `grant` whose tool calls the host checks against
    /// the grant's policy. Only a grant's turn is checked; the owner's never is.
    pub fn checked_grant(grant: GrantInitiator) -> Self {
        Self {
            initiator: Initiator::Grant(grant),
            tools: ToolChecking::Checked,
        }
    }
    /// A turn written by the agent of `source`; it does not inherit owner
    /// authority from the source turn.
    pub fn agent(source: AgentSource) -> Self {
        Self {
            initiator: Initiator::Agent(source),
            tools: ToolChecking::NotChecked,
        }
    }
    /// Who started the turn.
    pub fn initiator(&self) -> &Initiator {
        &self.initiator
    }
    /// Whether the turn's tools were checked.
    pub fn tools(&self) -> ToolChecking {
        self.tools
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_source_requires_a_canonical_conversation_identity() {
        let execution = ExecutionId::new("turn-1").unwrap();
        assert_eq!(
            AgentSource::new("   ", execution.clone()),
            Err(ExecutionError::EmptyValue("source conversation"))
        );
        assert_eq!(
            AgentSource::new(
                "x".repeat(AgentSource::MAX_CONVERSATION_BYTES + 1),
                execution.clone()
            ),
            Err(ExecutionError::ValueTooLong {
                field: "source conversation",
                max_bytes: AgentSource::MAX_CONVERSATION_BYTES,
            })
        );
        for conversation in [
            "not-a-conversation-id",
            "4C8B99B9-93E2-4E71-A131-83C8EA3EB7FA",
            "4c8b99b9-93e2-4e71-a131-83c8ea3eb7f",
        ] {
            assert_eq!(
                AgentSource::new(conversation, execution.clone()),
                Err(ExecutionError::InvalidAgentSource)
            );
        }
        let source =
            AgentSource::new("4c8b99b9-93e2-4e71-a131-83c8ea3eb7fa", execution.clone()).unwrap();
        assert_eq!(
            source.conversation(),
            "4c8b99b9-93e2-4e71-a131-83c8ea3eb7fa"
        );
        assert_eq!(source.execution(), &execution);
        assert_eq!(source.payload_bytes(), 36 + execution.as_str().len());
    }
}
