//! A cloud VM's auto-stop choice, shared by channel and workspace behavior.
//! Inheriting a default is absence of an override, not Unlimited.
use super::channel::ChannelId;
use std::num::NonZeroU32;

// Product protocol generation checks its published interval against these
// domain bounds. Boat's nullable u32 field constrains the finite representation.
const MIN_SECONDS: u64 = 1;
const MAX_SECONDS: u64 = 4_294_967_295;

/// How long a newly created or resumed VM runs before automatic stop.
/// Provider/account restrictions may still stop a VM sooner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VmLifetime {
    /// No Nessa-requested automatic stop; the user manages stopping the VM.
    #[default]
    Unlimited,
    /// A positive duration, including time spent on active work.
    After(NonZeroU32),
}
impl VmLifetime {
    /// Interpret explicit nullable seconds. `None` disables automatic stop.
    /// Zero and durations that Boat's u32 field cannot represent are refused.
    pub fn from_seconds(seconds: Option<u64>) -> Option<Self> {
        Some(match seconds {
            None => Self::Unlimited,
            Some(value) if (MIN_SECONDS..=MAX_SECONDS).contains(&value) => {
                Self::After(NonZeroU32::new(u32::try_from(value).ok()?)?)
            }
            Some(_) => return None,
        })
    }
    /// The provider value: an explicit null or positive seconds.
    pub const fn seconds(self) -> Option<u32> {
        match self {
            Self::Unlimited => None,
            Self::After(seconds) => Some(seconds.get()),
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/vm_lifetime.rs"]
mod tests;

/// The configuration target whose lifetime choices were observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmLifetimeScope {
    /// The Nessa default, without a channel override.
    Global,
    /// One stored channel's settings.
    Channel(ChannelId),
}
impl VmLifetimeScope {
    /// The channel identity, absent for the Nessa default.
    pub fn channel(&self) -> Option<&ChannelId> {
        match self {
            Self::Global => None,
            Self::Channel(channel) => Some(channel),
        }
    }
}

/// A default and optional override observed together for one target.
/// Absence means inheritance; explicit Unlimited remains independent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmLifetimeSettings {
    scope: VmLifetimeScope,
    default: VmLifetime,
    channel: Option<VmLifetime>,
}
impl VmLifetimeSettings {
    /// A global read cannot contain a channel override.
    pub const fn global(default: VmLifetime) -> Self {
        Self {
            scope: VmLifetimeScope::Global,
            default,
            channel: None,
        }
    }
    /// The observed default and override for this channel identity.
    pub fn for_channel(
        channel: ChannelId,
        default: VmLifetime,
        override_lifetime: Option<VmLifetime>,
    ) -> Self {
        Self {
            scope: VmLifetimeScope::Channel(channel),
            default,
            channel: override_lifetime,
        }
    }
    /// The target to compare with the initiating read or edit.
    pub fn scope(&self) -> &VmLifetimeScope {
        &self.scope
    }
    /// The observed Nessa default.
    pub const fn default_lifetime(&self) -> VmLifetime {
        self.default
    }
    /// The explicit channel choice, absent when it inherits.
    pub const fn channel_override(&self) -> Option<VmLifetime> {
        self.channel
    }
    /// The channel override, or the default when it inherits.
    pub const fn effective(&self) -> VmLifetime {
        match self.channel {
            Some(lifetime) => lifetime,
            None => self.default,
        }
    }
}
