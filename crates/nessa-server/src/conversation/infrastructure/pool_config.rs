//! The `pool` section of config.json: which paired computers new work goes
//! to first, which it never goes to, each channel's own choice, and when a
//! computer is too busy for new work. The shared ConfigJson reader validates
//! it with RuntimeConfig before any writer changes the file.
//!
//! ```json
//! "pool": {
//!   "order": ["peer:…", "peer:…"],
//!   "excluded": ["peer:…"],
//!   "channels": {"ch_…": {"order": ["peer:…"], "excluded": []}},
//!   "busyCpuPercent": 90,
//!   "busyMemoryPercent": 90
//! }
//! ```
//!
//! Absent, every connected computer is in the pool, none goes first, and a
//! computer at 90% processor or memory use is busy.
#![cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
use crate::conversation::domain::{
    ChannelId, PoolPreference, PoolSettings, SettingsScope, MAX_POOL_ENTRIES,
};
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use serde::{de, Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The processor or memory use, in percent, at which a computer is busy
/// when the configuration names none.
pub(crate) const DEFAULT_BUSY_PERCENT: u32 = 90;

/// Validated default, overrides and limits from one configuration document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolConfig {
    default: PoolPreference,
    channels: BTreeMap<ChannelId, PoolPreference>,
    busy_cpu_percent: u32,
    busy_memory_percent: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            default: PoolPreference::default(),
            channels: BTreeMap::new(),
            busy_cpu_percent: DEFAULT_BUSY_PERCENT,
            busy_memory_percent: DEFAULT_BUSY_PERCENT,
        }
    }
}

impl PoolConfig {
    /// Read only this section of a whole configuration document.
    pub(crate) fn from_document(document: &Map<String, Value>) -> Result<Self, serde_json::Error> {
        match document.get("pool") {
            None => Ok(Self::default()),
            Some(section) => serde_json::from_value(section.clone()),
        }
    }

    /// The default, `scope`'s override, and the limits, as one read.
    pub(crate) fn snapshot(&self, scope: &SettingsScope) -> PoolSettings {
        PoolSettings::new(
            scope.clone(),
            self.default.clone(),
            scope
                .channel()
                .and_then(|channel| self.channels.get(channel).cloned()),
            self.busy_cpu_percent * 10,
            self.busy_memory_percent * 10,
        )
    }

    /// `scope`'s own choice: the default for the Nessa default, which is
    /// always set, or the channel's override, absent when it inherits.
    pub(crate) fn chosen(&self, scope: &SettingsScope) -> Option<PoolPreference> {
        match scope {
            SettingsScope::Global => Some(self.default.clone()),
            SettingsScope::Channel(channel) => self.channels.get(channel).cloned(),
        }
    }
}

fn names(peers: &[PeerEnvironment]) -> Value {
    Value::Array(
        peers
            .iter()
            .map(|peer| Value::String(peer.name()))
            .collect(),
    )
}

fn preference_value(preference: &PoolPreference) -> Value {
    serde_json::json!({"order": names(preference.order()), "excluded": names(preference.excluded())})
}

/// Write `choice` for `scope` into a whole configuration document: the
/// default's order and left-out list, or a channel's override, which `None`
/// removes so the channel inherits again. `None` for the default empties it.
/// Every other key, in this section and outside it, is kept as it was.
pub(crate) fn edit(
    document: &mut Map<String, Value>,
    scope: &SettingsScope,
    choice: Option<&PoolPreference>,
) -> Result<(), ()> {
    let pool = document
        .entry("pool")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(())?;
    match scope {
        SettingsScope::Global => {
            let preference = choice.cloned().unwrap_or_default();
            pool.insert("order".into(), names(preference.order()));
            pool.insert("excluded".into(), names(preference.excluded()));
        }
        SettingsScope::Channel(channel) => {
            let channels = pool
                .entry("channels")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or(())?;
            match choice {
                Some(preference) => {
                    channels.insert(channel.as_str().into(), preference_value(preference));
                }
                None => {
                    channels.remove(channel.as_str());
                }
            }
        }
    }
    Ok(())
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WirePreference {
    order: Vec<String>,
    excluded: Vec<String>,
}

impl WirePreference {
    fn preference<E: de::Error>(self) -> Result<PoolPreference, E> {
        let parse = |names: Vec<String>| {
            if names.len() > MAX_POOL_ENTRIES {
                return Err(E::custom("too many computers"));
            }
            names
                .iter()
                .map(|name| PeerEnvironment::parse(name).map_err(|_| E::custom("invalid computer")))
                .collect::<Result<Vec<_>, E>>()
        };
        PoolPreference::new(parse(self.order)?, parse(self.excluded)?)
            .map_err(|_| E::custom("invalid pool preference"))
    }
}

impl<'de> Deserialize<'de> for PoolConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields, rename_all = "camelCase")]
        struct Wire {
            order: Vec<String>,
            excluded: Vec<String>,
            #[serde(deserialize_with = "overrides")]
            channels: BTreeMap<ChannelId, PoolPreference>,
            busy_cpu_percent: u32,
            busy_memory_percent: u32,
        }
        impl Default for Wire {
            fn default() -> Self {
                Self {
                    order: Vec::new(),
                    excluded: Vec::new(),
                    channels: BTreeMap::new(),
                    busy_cpu_percent: DEFAULT_BUSY_PERCENT,
                    busy_memory_percent: DEFAULT_BUSY_PERCENT,
                }
            }
        }
        let wire = Wire::deserialize(deserializer)?;
        let percent = |value: u32| {
            if (1..=100).contains(&value) {
                Ok(value)
            } else {
                Err(de::Error::custom("a busy limit is a percent from 1 to 100"))
            }
        };
        Ok(Self {
            default: WirePreference {
                order: wire.order,
                excluded: wire.excluded,
            }
            .preference()?,
            channels: wire.channels,
            busy_cpu_percent: percent(wire.busy_cpu_percent)?,
            busy_memory_percent: percent(wire.busy_memory_percent)?,
        })
    }
}

fn overrides<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<ChannelId, PoolPreference>, D::Error> {
    struct Unique;
    impl<'de> de::Visitor<'de> for Unique {
        type Value = BTreeMap<ChannelId, PoolPreference>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("unique channel pool preferences")
        }
        fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut channels = BTreeMap::new();
            while let Some((id, wire)) = map.next_entry::<String, WirePreference>()? {
                let id = ChannelId::new(&id).map_err(|_| de::Error::custom("invalid channel"))?;
                if id.is_default() || channels.contains_key(&id) {
                    return Err(de::Error::custom("invalid channel override"));
                }
                channels.insert(id, wire.preference()?);
            }
            Ok(channels)
        }
    }
    deserializer.deserialize_map(Unique)
}

#[cfg(test)]
#[path = "../../../tests/conversation/pool_config.rs"]
mod tests;
