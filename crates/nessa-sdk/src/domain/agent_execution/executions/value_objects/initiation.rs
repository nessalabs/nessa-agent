//! Who started a turn, and whether its tools were checked against a policy.
//! The host decides both before submission; these values only carry the answer
//! so every saved turn says it, and a retry can be compared with the original.
#![deny(missing_docs)]

use crate::domain::agent_execution::ExecutionError;

/// Access a share grant gives the person who holds it.
/// This is the SDK's own copy of the role a host checked; the SDK never reads
/// the host's share records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantRole {
    /// May read the conversation.
    Read,
    /// May read and comment.
    Comment,
    /// May drive the conversation: start and steer turns.
    Drive,
}

/// Who started a turn: the conversation's owner, or someone holding a share grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Initiator {
    /// The conversation's owner started the turn.
    Owner,
    /// Someone holding a share grant started the turn.
    Grant(GrantInitiator),
}

/// The share grant a turn was started under, as the host checked it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantInitiator {
    grant: Box<str>,
    role: GrantRole,
    policy_revision: u64,
}
impl GrantInitiator {
    /// Maximum UTF-8 byte length of a grant's identity text.
    pub const MAX_GRANT_BYTES: usize = 256;

    /// Retain the grant's exact identity text, the role it gave, and the
    /// revision of the grant's policy the host checked. Blank `grant` text
    /// returns [`ExecutionError::EmptyValue`]; text over
    /// [`Self::MAX_GRANT_BYTES`] UTF-8 bytes returns
    /// [`ExecutionError::ValueTooLong`]. This does not check the grant exists.
    pub fn new(
        grant: impl Into<String>,
        role: GrantRole,
        policy_revision: u64,
    ) -> Result<Self, ExecutionError> {
        let grant = grant.into();
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
            role,
            policy_revision,
        })
    }
    /// The grant's exact identity text.
    pub fn grant(&self) -> &str {
        &self.grant
    }
    /// The role the grant gave when the turn started.
    pub fn role(&self) -> GrantRole {
        self.role
    }
    /// Revision of the grant's policy the host checked.
    pub fn policy_revision(&self) -> u64 {
        self.policy_revision
    }
}

/// Whether a turn's tools were checked against a policy before they ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolChecking {
    /// The turn's tools ran without a policy check. A later slice (J4 of #707)
    /// adds the enforced case; until then every turn's tools run unchecked
    /// (decided on #707, 2026-10-12).
    NotChecked,
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
    /// Who started the turn.
    pub fn initiator(&self) -> &Initiator {
        &self.initiator
    }
    /// Whether the turn's tools were checked.
    pub fn tools(&self) -> ToolChecking {
        self.tools
    }
}
