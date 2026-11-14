//! Which settings a read or an edit names: the Nessa default, or one stored
//! channel's override of it. Shared by every setting a channel inherits.
use super::channel::ChannelId;

/// The configuration target whose choices were observed or edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsScope {
    /// The Nessa default, without a channel override.
    Global,
    /// One stored channel's settings.
    Channel(ChannelId),
}
impl SettingsScope {
    /// The channel identity, absent for the Nessa default.
    pub fn channel(&self) -> Option<&ChannelId> {
        match self {
            Self::Global => None,
            Self::Channel(channel) => Some(channel),
        }
    }
}
