use std::fmt;

/// Why a string is not an agent name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAnAgentName(String);

impl NotAnAgentName {
    /// What was offered, for a diagnostic.
    pub fn offered(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NotAnAgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "an agent name is lowercase letters, digits and hyphens: {:?}",
            self.0
        )
    }
}

impl std::error::Error for NotAnAgentName {}

/// The name an agent is known by when Nessa installs its runtime.
///
/// A value object rather than a `&str` because the name becomes a directory
/// under Nessa's own data directory. A name carrying a separator, a `..`, or a
/// leading `/` would put a downloaded runtime somewhere else entirely, so the
/// shape is settled once, here, and every layer downstream holds a name that
/// already is one.
///
/// Deliberately open, and deliberately not [`crate::agents::domain::AgentId`]:
/// that enum lists the agents this build can *drive*, and installing is not
/// driving. A pin can exist for an agent no adapter has been written for yet —
/// that is the state an agent passes through while it is being added — and a
/// person can type a name for an agent that does not exist at all. Both are
/// answered by looking for a pin, which is a fact about the release data rather
/// than about the shape of the string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentName(String);

impl AgentName {
    /// Read a name, rejecting anything that could not be a directory name.
    ///
    /// Lowercase letters, digits and hyphens only. Narrower than the filesystem
    /// requires on purpose: the set of things an agent is actually called is
    /// small, and every character outside it is more likely a typo or an attempt
    /// to escape the directory than a real name.
    pub fn parse(value: &str) -> Result<Self, NotAnAgentName> {
        let plain = !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if plain {
            Ok(Self(value.to_owned()))
        } else {
            Err(NotAnAgentName(value.to_owned()))
        }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_install/agent_name.rs"]
mod tests;
