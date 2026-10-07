//! The `agents.mcpServers` block of `config.json`, the one reader and writer
//! of its shape: what `AgentsConfig` parses at startup and what
//! `mcpServers.save` and `mcpServers.remove` write back.
//!
//! An entry is `{name, command, args?, enabled?, env?}`. One without
//! `enabled` is on, and one without `env` has no variables of its own: that
//! is the current contract's default, not a reading of an older shape.
//!
//! `env` is read entry by entry, every entry kept, and handed to
//! [`ConfiguredMcpServer::new`], which refuses a name given twice: decoded
//! into a map first, the second value would silently replace the first
//! (`a_repeated_variable_name_in_the_file_is_refused_in_either_order`).
//!
//! A name longer than the SDK allows ([`MAX_MCP_SERVER_NAME_BYTES`]) makes
//! the block unreadable, as any other hand edit that does not parse: every
//! other rule for a server is the SDK's, asked of the whole list and named
//! in `mcp_servers_invalid`, but a name past this bound would not fit in the
//! frame of a request that names it — a remove, an inspection — so the
//! server could not be taken out through the gateway
//! (`a_stored_name_past_the_sdks_bound_makes_the_configuration_invalid`).
use crate::mcp_servers::domain::{
    stored_revision, ConfigurationKey, ConfiguredMcpServer, RemoteConfigured, StdioServer,
    StoredMcpServer,
};
use nessa_sdk::infrastructure::acp::sessions::MAX_MCP_SERVER_NAME_BYTES;
use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::{collections::BTreeMap, fmt, path::PathBuf};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredFileServer {
    name: String,
    command: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default = "enabled")]
    enabled: bool,
    #[serde(default)]
    env: Entries,
}

/// An entry as it is written: its variables by name, as the server holds
/// them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WrittenMcpServer<'a> {
    name: &'a str,
    command: &'a std::path::Path,
    args: &'a [String],
    enabled: bool,
    env: &'a BTreeMap<String, String>,
}

/// An `env` object's entries in the order the file gives them, a name
/// given twice kept twice.
#[derive(Default)]
struct Entries(Vec<(String, String)>);
impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Each;
        impl<'de> Visitor<'de> for Each {
            type Value = Entries;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of variable names and string values")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, String>()? {
                    entries.push(entry);
                }
                Ok(Entries(entries))
            }
        }
        deserializer.deserialize_map(Each)
    }
}

fn enabled() -> bool {
    true
}

impl StoredFileServer {
    /// The entry at `index` as the gateway holds it, or why it cannot be: a
    /// name past [`MAX_MCP_SERVER_NAME_BYTES`], or a variable named twice.
    /// A name past the bound is logged by its entry's index and its length,
    /// since `config_invalid` carries no details to say which entry it was
    /// (`a_stored_name_past_the_sdks_bound_makes_the_configuration_invalid`).
    fn configured(self, index: usize) -> Result<ConfiguredMcpServer, String> {
        check_name(&self.name, index)?;
        let name = self.name.clone();
        ConfiguredMcpServer::new(
            StdioServer::new(self.name, self.command, self.args),
            self.enabled,
            self.env.0,
        )
        .map_err(|repeated| {
            format!(
                "MCP server {name:?} names the variable {:?} twice",
                repeated.name
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredRemote {
    kind: String,
    id: String,
    name: String,
    url: String,
    #[serde(default = "enabled")]
    enabled: bool,
}

/// Parse an `agents.mcpServers` block: `#[serde(deserialize_with)]` for
/// `AgentsConfig`. A stdio entry has no `kind`. A remote entry is
/// `{kind: "remote", id, name, url, enabled?}`.
pub fn stored_servers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<StoredMcpServer>, D::Error> {
    // The entry's own JSON, not a `Value`: a map decoded first keeps one
    // value when a name is given twice, and the repetition would be lost
    // (`a_repeated_variable_name_in_the_file_is_refused_in_either_order`).
    let values = Vec::<Box<serde_json::value::RawValue>>::deserialize(deserializer)?;
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| parse_entry(value.get(), index))
        .collect::<Result<_, _>>()
        .map_err(serde::de::Error::custom)
}

#[derive(Deserialize)]
struct KindProbe {
    kind: Option<String>,
}

fn parse_entry(raw: &str, index: usize) -> Result<StoredMcpServer, String> {
    let kind = serde_json::from_str::<KindProbe>(raw)
        .map_err(|error| error.to_string())?
        .kind;
    match kind.as_deref() {
        Some("remote") => {
            let stored: StoredRemote =
                serde_json::from_str(raw).map_err(|error| error.to_string())?;
            if stored.kind != "remote" {
                return Err(format!("agents.mcpServers entry {index} is not remote"));
            }
            check_name(&stored.name, index)?;
            let id = uuid::Uuid::parse_str(&stored.id).map_err(|_| {
                format!("agents.mcpServers entry {index} has an id that is not a UUID")
            })?;
            Ok(StoredMcpServer::Remote(RemoteConfigured::new(
                id,
                stored.name,
                stored.url,
                stored.enabled,
            )))
        }
        Some(_) => Err(format!(
            "agents.mcpServers entry {index} has an unknown kind"
        )),
        None => {
            let stored: StoredFileServer =
                serde_json::from_str(raw).map_err(|error| error.to_string())?;
            stored.configured(index).map(StoredMcpServer::Stdio)
        }
    }
}

fn check_name(name: &str, index: usize) -> Result<(), String> {
    let bytes = name.len();
    if bytes > MAX_MCP_SERVER_NAME_BYTES {
        tracing::warn!(
            index,
            bytes,
            max = MAX_MCP_SERVER_NAME_BYTES,
            "agents.mcpServers entry's name is past the bound; the configuration is refused"
        );
        return Err(format!(
            "the MCP server name of agents.mcpServers entry {index} is {bytes} bytes, \
             past the {MAX_MCP_SERVER_NAME_BYTES} allowed"
        ));
    }
    Ok(())
}

/// `block` (an `agents.mcpServers` value) parsed, or `None` when it is not
/// one.
pub(crate) fn parse_block(block: &Value) -> Option<Vec<StoredMcpServer>> {
    stored_servers(block).ok()
}

/// `servers` as an `agents.mcpServers` block, every field written; `None`
/// when a command is not UTF-8, which the SDK's rules refuse first
/// (`McpServerProblem::Command`).
pub(crate) fn block(servers: &[StoredMcpServer]) -> Option<Value> {
    let mut stored = Vec::with_capacity(servers.len());
    for server in servers {
        let value = match server {
            StoredMcpServer::Stdio(configured) => serde_json::to_value(WrittenMcpServer {
                name: configured.server().name(),
                command: configured.server().command(),
                args: configured.server().args(),
                enabled: configured.enabled(),
                env: configured.env(),
            })
            .ok()?,
            StoredMcpServer::Remote(remote) => serde_json::json!({
                "kind": "remote",
                "id": remote.id().to_string(),
                "name": remote.name(),
                "url": remote.url(),
                "enabled": remote.enabled(),
            }),
        };
        stored.push(value);
    }
    Some(Value::Array(stored))
}

/// The revision of a stored block: a digest of its JSON keyed with this
/// process's `key` ([`stored_revision`]), an absent block being `[]`.
/// Nothing beside the block is persisted for it.
pub(crate) fn revision(key: &ConfigurationKey, block: Option<&Value>) -> String {
    let empty = Value::Array(Vec::new());
    let bytes = serde_json::to_vec(block.unwrap_or(&empty)).unwrap_or_default();
    stored_revision(key, &bytes)
}
