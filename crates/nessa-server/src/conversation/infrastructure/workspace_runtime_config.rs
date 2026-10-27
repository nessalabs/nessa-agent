//! The ordinary Boat lifetime section of config.json. The shared ConfigJson
//! reader validates it with RuntimeConfig before any writer changes the file.
#[cfg(any(unix, test))]
use crate::conversation::domain::VmLifetimeSettings;
use crate::conversation::domain::{ChannelId, VmLifetime, MAX_CHANNELS_PER_OWNER};
use serde::{de, Deserialize, Deserializer};
use serde_json::{Error, Map, Value};
use std::collections::BTreeMap;

/// Validated default and overrides from one configuration document.
#[derive(Debug, Default)]
pub(crate) struct BoatRuntimeConfig {
    default: Option<VmLifetime>,
    channels: BTreeMap<ChannelId, VmLifetime>,
}
impl BoatRuntimeConfig {
    /// Read only this section while retaining raw duplicate-key detection.
    /// Other runtime sections are owned by their existing readers.
    pub(crate) fn from_document(bytes: &[u8]) -> Result<Self, Error> {
        #[derive(Deserialize)]
        struct Document {
            #[serde(default)]
            boat: BoatRuntimeConfig,
        }
        serde_json::from_slice::<Document>(bytes).map(|document| document.boat)
    }
    /// Resolve both values from the same configuration document.
    #[cfg(any(unix, test))]
    pub(crate) fn snapshot(&self, channel: Option<&ChannelId>) -> VmLifetimeSettings {
        match channel {
            None => VmLifetimeSettings::global(self.default_lifetime()),
            Some(channel) => VmLifetimeSettings::for_channel(
                channel.clone(),
                self.default_lifetime(),
                self.channels.get(channel).copied(),
            ),
        }
    }
    /// Only validated non-secret settings are included in a backup record.
    pub(crate) fn section(&self) -> Value {
        let channels = self
            .channels
            .iter()
            .map(|(channel, lifetime)| {
                (
                    channel.as_str().to_owned(),
                    serde_json::json!(lifetime.seconds()),
                )
            })
            .collect::<Map<String, Value>>();
        let mut value = serde_json::json!({"channelAutoStopSeconds":channels});
        if let Some(default) = self.default {
            value.as_object_mut().expect("object literal").insert(
                "autoStopSeconds".into(),
                serde_json::json!(default.seconds()),
            );
        }
        value
    }
    #[cfg(any(unix, test))]
    pub(crate) fn default_lifetime(&self) -> VmLifetime {
        self.default.unwrap_or_default()
    }
    #[cfg(any(unix, test))]
    pub(crate) fn default_override(&self) -> Option<VmLifetime> {
        self.default
    }
    #[cfg(any(unix, test))]
    pub(crate) fn channel_override(&self, channel: &ChannelId) -> Option<VmLifetime> {
        self.channels.get(channel).copied()
    }
}
impl<'de> Deserialize<'de> for BoatRuntimeConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields, rename_all = "camelCase")]
        struct Wire {
            #[serde(deserialize_with = "explicit_lifetime")]
            auto_stop_seconds: Option<VmLifetime>,
            #[serde(deserialize_with = "overrides")]
            channel_auto_stop_seconds: BTreeMap<ChannelId, VmLifetime>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            default: wire.auto_stop_seconds,
            channels: wire.channel_auto_stop_seconds,
        })
    }
}
fn explicit_lifetime<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<VmLifetime>, D::Error> {
    let seconds = Option::<u64>::deserialize(deserializer)?;
    VmLifetime::from_seconds(seconds)
        .map(Some)
        .ok_or_else(|| de::Error::custom("invalid VM lifetime"))
}
fn overrides<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<ChannelId, VmLifetime>, D::Error> {
    struct Unique;
    impl<'de> de::Visitor<'de> for Unique {
        type Value = BTreeMap<ChannelId, VmLifetime>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("unique channel lifetime overrides")
        }
        fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut channels = BTreeMap::new();
            while let Some((id, seconds)) = map.next_entry::<String, Option<u64>>()? {
                let id = ChannelId::new(&id).map_err(|_| de::Error::custom("invalid channel"))?;
                if id.is_default() || channels.contains_key(&id) {
                    return Err(de::Error::custom("invalid channel override"));
                }
                if channels.len() == MAX_CHANNELS_PER_OWNER {
                    return Err(de::Error::custom("too many channel overrides"));
                }
                let lifetime = VmLifetime::from_seconds(seconds)
                    .ok_or_else(|| de::Error::custom("invalid VM lifetime"))?;
                channels.insert(id, lifetime);
            }
            Ok(channels)
        }
    }
    deserializer.deserialize_map(Unique)
}

#[cfg(test)]
#[path = "../../../tests/conversation/workspace_runtime_config.rs"]
mod tests;
