//! The `agents.mcpServers` block of `config.json`, the one reader and writer
//! of its shape: what `AgentsConfig` parses at startup and what
//! `mcpServers.save` and `mcpServers.remove` write back.
//!
//! An entry is `{name, command, args?, enabled?, env?}`. One without
//! `enabled` is on, and one without `env` has no variables of its own: that
//! is the current contract's default, not a reading of an older shape.
use crate::mcp_servers::domain::{ConfiguredMcpServer, StdioServer};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredMcpServer {
    name: String,
    command: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default = "enabled")]
    enabled: bool,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

fn enabled() -> bool {
    true
}

impl From<StoredMcpServer> for ConfiguredMcpServer {
    fn from(stored: StoredMcpServer) -> Self {
        Self {
            server: StdioServer {
                name: stored.name,
                command: stored.command,
                args: stored.args,
            },
            enabled: stored.enabled,
            env: stored.env,
        }
    }
}

/// Parse an `agents.mcpServers` block: `#[serde(deserialize_with)]` for
/// `AgentsConfig`.
pub fn stored_servers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<ConfiguredMcpServer>, D::Error> {
    let stored = Vec::<StoredMcpServer>::deserialize(deserializer)?;
    Ok(stored.into_iter().map(ConfiguredMcpServer::from).collect())
}

/// `block` (an `agents.mcpServers` value) parsed, or `None` when it is not
/// one.
pub(crate) fn parse_block(block: &Value) -> Option<Vec<ConfiguredMcpServer>> {
    stored_servers(block).ok()
}

/// `servers` as an `agents.mcpServers` block, every field written; `None`
/// when a command is not UTF-8, which the SDK's rules refuse first
/// (`McpServerProblem::Command`).
pub(crate) fn block(servers: &[ConfiguredMcpServer]) -> Option<Value> {
    let stored: Vec<StoredMcpServer> = servers
        .iter()
        .map(|configured| StoredMcpServer {
            name: configured.server.name.clone(),
            command: configured.server.command.clone(),
            args: configured.server.args.clone(),
            enabled: configured.enabled,
            env: configured.env.clone(),
        })
        .collect();
    serde_json::to_value(stored).ok()
}

/// The revision of a stored block: a digest of its JSON, an absent block
/// being `[]`. Nothing beside the block is persisted for it.
pub(crate) fn revision(block: Option<&Value>) -> String {
    let empty = Value::Array(Vec::new());
    let bytes = serde_json::to_vec(block.unwrap_or(&empty)).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(bytes))
}
