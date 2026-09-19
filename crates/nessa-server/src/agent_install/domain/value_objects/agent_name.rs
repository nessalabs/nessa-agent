use std::fmt;

use super::device_names::names_a_device;

/// Why a string is not an agent name.
///
/// One variant per rule rather than one message for all of them. `parse`
/// enforces four things, and the only thing a person gets back is this line: a
/// message naming rules the input already satisfies tells them nothing and
/// reads as a contradiction. Somebody who typed sixty-five lowercase letters
/// should be told about the length, not about the alphabet they used correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAnAgentName {
    /// Nothing was offered.
    Empty,
    /// Longer than a name has any reason to be.
    TooLong(String),
    /// Something outside lowercase letters, digits and hyphens.
    Spelling(String),
    /// A name Windows answers to as a device rather than as a directory.
    ReservedDevice(String),
}

impl NotAnAgentName {
    /// What was offered, for a diagnostic.
    pub fn offered(&self) -> &str {
        match self {
            Self::Empty => "",
            Self::TooLong(value) | Self::Spelling(value) | Self::ReservedDevice(value) => value,
        }
    }
}

impl fmt::Display for NotAnAgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("an agent name is needed, such as opencode"),
            Self::TooLong(value) => write!(
                f,
                "an agent name is at most {MAXIMUM_NAME_LENGTH} characters: {value:?}"
            ),
            Self::Spelling(value) => write!(
                f,
                "an agent name is lowercase letters, digits and hyphens: {value:?}"
            ),
            Self::ReservedDevice(value) => write!(
                f,
                "an agent name cannot be one windows keeps for a device: {value:?}"
            ),
        }
    }
}

/// The longest an agent name may be.
///
/// A name becomes one path component, and every filesystem stops somewhere
/// around 255 bytes. Far below that, because a name longer than this is not the
/// name of an agent.
const MAXIMUM_NAME_LENGTH: usize = 64;

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
    ///
    /// The device names Windows reserves are refused too, for the same reason
    /// [`super::ReleaseVersion`] refuses them: both become a path component, and
    /// a name that is a directory here and a device there is not one this domain
    /// can accept. The rule is shared rather than restated so the two cannot
    /// come apart.
    pub fn parse(value: &str) -> Result<Self, NotAnAgentName> {
        if value.is_empty() {
            return Err(NotAnAgentName::Empty);
        }
        if value.len() > MAXIMUM_NAME_LENGTH {
            return Err(NotAnAgentName::TooLong(value.to_owned()));
        }
        if names_a_device(value) {
            return Err(NotAnAgentName::ReservedDevice(value.to_owned()));
        }
        let plain = value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if plain {
            Ok(Self(value.to_owned()))
        } else {
            Err(NotAnAgentName::Spelling(value.to_owned()))
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
