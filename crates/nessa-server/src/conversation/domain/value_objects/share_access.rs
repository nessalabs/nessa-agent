//! What a share grant lets its holder do (issue 707): a role, and for Drive
//! the tool policy that holder's turns will run under.
//!
//! This value checks shape and size only. What a policy allows is decided
//! by the application's `ToolPolicyEvaluator`, which Cedar answers; this
//! value never matches a pattern itself.
//!
//! ```text
//! ShareAccess ──role──▶ ShareRole (Read, Comment or Drive)
//!      └──────policy──▶ ToolPolicy (Drive only) ──▶ ToolPreset | ToolRules
//! ```

/// How much a share lets its holder do. What each role allows is the
/// conversation access policy's, which Cedar answers; this value only names
/// the role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShareRole {
    /// Fold the records and watch them live.
    Read,
    /// Read, and leave notes the agent sees on the owner's next turn.
    Comment,
    /// Comment, and start, steer and stop turns under a tool policy.
    Drive,
}

/// A named tool policy a Drive grant may use instead of its own rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolPreset {
    /// Only tools that read.
    ReadOnlyTools,
    /// Every tool except shell commands.
    NoShell,
    /// Every tool, each one asked for.
    AskEverything,
    /// Every tool, asked as the conversation's approval mode asks.
    All,
}

/// Allow and deny patterns for the tools a Drive grant's turns may call. A
/// pattern is kept as written; what it matches is decided where the policy
/// is enforced, not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRules {
    allow: Vec<String>,
    deny: Vec<String>,
}

impl ToolRules {
    /// The most patterns each list holds: allow, and separately deny.
    pub const MAX_RULES: usize = 16;
    /// The longest pattern, in UTF-8 bytes.
    pub const MAX_RULE_BYTES: usize = 48;

    /// Rules from `allow` and `deny`, refused when both are empty (a Drive
    /// grant's policy must say something), when either holds more than
    /// [`Self::MAX_RULES`] patterns, or when a pattern is empty, longer than
    /// [`Self::MAX_RULE_BYTES`], has surrounding whitespace, or holds a
    /// control character.
    pub fn new(allow: Vec<String>, deny: Vec<String>) -> Result<Self, ToolPolicyError> {
        if allow.is_empty() && deny.is_empty() {
            return Err(ToolPolicyError::NoRules);
        }
        if allow.len() > Self::MAX_RULES || deny.len() > Self::MAX_RULES {
            return Err(ToolPolicyError::TooManyRules);
        }
        for rule in allow.iter().chain(&deny) {
            if rule.is_empty() || rule.trim() != rule || rule.chars().any(char::is_control) {
                return Err(ToolPolicyError::MalformedRule);
            }
            if rule.len() > Self::MAX_RULE_BYTES {
                return Err(ToolPolicyError::RuleTooLong);
            }
        }
        Ok(Self { allow, deny })
    }

    /// The allow patterns, in the order they were given.
    pub fn allow(&self) -> &[String] {
        &self.allow
    }

    /// The deny patterns, in the order they were given.
    pub fn deny(&self) -> &[String] {
        &self.deny
    }
}

/// The tool policy a Drive grant's turns run under: a preset, or rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolPolicy {
    Preset(ToolPreset),
    Rules(ToolRules),
}

/// Why a set of tool rules was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPolicyError {
    /// Neither an allow nor a deny pattern.
    NoRules,
    /// More than [`ToolRules::MAX_RULES`] patterns in allow, or in deny.
    TooManyRules,
    /// A pattern longer than [`ToolRules::MAX_RULE_BYTES`].
    RuleTooLong,
    /// An empty pattern, one with surrounding whitespace, or one holding a
    /// control character.
    MalformedRule,
}

/// A role with the policy it needs: a Drive grant always has a tool policy,
/// and a Read or Comment grant never has one, because neither starts a turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareAccess {
    role: ShareRole,
    policy: Option<ToolPolicy>,
}

impl ShareAccess {
    /// `role` with `policy`, refused when a Drive grant names no policy or
    /// a Read or Comment grant names one.
    pub fn new(role: ShareRole, policy: Option<ToolPolicy>) -> Result<Self, ShareAccessError> {
        match (role, &policy) {
            (ShareRole::Drive, None) => Err(ShareAccessError::PolicyRequired),
            (ShareRole::Read | ShareRole::Comment, Some(_)) => {
                Err(ShareAccessError::PolicyNotAllowed)
            }
            _ => Ok(Self { role, policy }),
        }
    }

    /// Read, which takes no policy.
    pub fn read() -> Self {
        Self {
            role: ShareRole::Read,
            policy: None,
        }
    }

    pub fn role(&self) -> ShareRole {
        self.role
    }

    /// The tool policy, present exactly when the role is Drive.
    pub fn policy(&self) -> Option<&ToolPolicy> {
        self.policy.as_ref()
    }
}

/// Why a role and policy were refused together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareAccessError {
    /// Drive names no tool policy.
    PolicyRequired,
    /// Read or Comment names a tool policy.
    PolicyNotAllowed,
}

#[cfg(test)]
#[path = "../../../../tests/conversation/share_access.rs"]
mod tests;
