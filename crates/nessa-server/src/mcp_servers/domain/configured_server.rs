//! A user's MCP server as the gateway stores it — the SDK's server, whether it
//! is on, and the variables it is given — and the edits `mcpServers.save` and
//! `mcpServers.remove` make to the stored list.
//!
//! ```text
//! stored list ──ServerEdit::apply──▶ edited list (or an EditRefusal)
//!                 │ save: upsert by name, rename by previous name, keep a
//!                 │       stored value for each variable given without one
//!                 └ remove: by name
//! ```
//!
//! The SDK's rules for a server and a set (`StdioMcpServer::problem`,
//! `McpServerLaunch::problem_in`) are not repeated here: the application asks
//! them of the edited list, through its live set port. This owns only what
//! the SDK does not know — the reserved name, which entry an edit names, and
//! a kept value.
use std::{collections::BTreeMap, path::PathBuf};

/// The name of the server Nessa manages itself: the desktop's bundled one
/// (`composition::desktop`). No edit names it, and `mcpServers.list` marks it
/// managed.
pub const MANAGED_SERVER_NAME: &str = "nessa";

/// A stdio server: what the SDK's `StdioMcpServer` is, as this context holds
/// it (`infrastructure` converts between them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdioServer {
    /// Its name, unique among the configured servers.
    pub name: String,
    /// The executable it is started as.
    pub command: PathBuf,
    /// Its arguments, in order.
    pub args: Vec<String>,
}

/// One server as `config.json` stores it under `agents.mcpServers`.
///
/// `Debug` names the variables in `env` and never prints a value, which may be
/// a credential (`a_configured_server_prints_its_variable_names_never_their_values`).
#[derive(Clone, PartialEq, Eq)]
pub struct ConfiguredMcpServer {
    /// The server: name, absolute executable, arguments.
    pub server: StdioServer,
    /// Whether it is in the launch set. A server turned off stays stored.
    pub enabled: bool,
    /// The variables it is given over the gateway's own; a value here wins.
    pub env: BTreeMap<String, String>,
}
impl ConfiguredMcpServer {
    /// Whether this is the server Nessa manages ([`MANAGED_SERVER_NAME`]).
    pub fn managed(&self) -> bool {
        self.server.name == MANAGED_SERVER_NAME
    }
    /// The names of its variables, in order.
    pub fn env_names(&self) -> Vec<String> {
        self.env.keys().cloned().collect()
    }
}
impl std::fmt::Debug for ConfiguredMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfiguredMcpServer")
            .field("server", &self.server)
            .field("enabled", &self.enabled)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// What `mcpServers.save` asks for: the server as it should be stored, under
/// its own name, replacing the one stored as `previous_name` when that is set.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerSave {
    /// The name it is stored under now, when it is being renamed.
    pub previous_name: Option<String>,
    /// The server as it should be.
    pub server: StdioServer,
    /// Each variable, in order: its value, or `None` to keep the value
    /// stored for that name.
    pub env: Vec<(String, Option<String>)>,
    /// Whether it should be in the launch set.
    pub enabled: bool,
}
impl std::fmt::Debug for ServerSave {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerSave")
            .field("previous_name", &self.previous_name)
            .field("server", &self.server)
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// One change to the stored list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEdit {
    /// Store a server, adding it or replacing the one under its name (or
    /// under its previous name).
    Save(ServerSave),
    /// Take the server stored under `name` out of the list.
    Remove {
        /// The stored name.
        name: String,
    },
}

/// Why an edit cannot be made to the stored list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditRefusal {
    /// It names [`MANAGED_SERVER_NAME`].
    ReservedName,
    /// No server is stored under the name it edits.
    NotFound,
    /// A variable is given without a value, to keep the stored one, and none
    /// is stored for that name.
    EnvironmentValueMissing {
        /// The variable's name.
        name: String,
    },
    /// A variable is given twice.
    EnvironmentNameRepeated {
        /// The variable's name.
        name: String,
    },
}

impl ServerEdit {
    /// The name this edit stores or removes.
    pub fn target(&self) -> &str {
        match self {
            Self::Save(save) => &save.server.name,
            Self::Remove { name } => name,
        }
    }

    /// `stored` with this edit made: a saved server takes the place of the
    /// one it replaces, or goes last; a removed one is left out. Nothing
    /// about the result as a set is checked here.
    ///
    /// # Errors
    ///
    /// [`EditRefusal`]: a reserved name, a name not stored, or a variable kept
    /// that has no stored value, or given twice.
    pub fn apply(
        &self,
        stored: &[ConfiguredMcpServer],
    ) -> Result<Vec<ConfiguredMcpServer>, EditRefusal> {
        let position = |name: &str| stored.iter().position(|each| each.server.name == name);
        match self {
            Self::Remove { name } => {
                if name == MANAGED_SERVER_NAME {
                    return Err(EditRefusal::ReservedName);
                }
                let found = position(name).ok_or(EditRefusal::NotFound)?;
                let mut edited = stored.to_vec();
                edited.remove(found);
                Ok(edited)
            }
            Self::Save(save) => {
                if save.server.name == MANAGED_SERVER_NAME
                    || save.previous_name.as_deref() == Some(MANAGED_SERVER_NAME)
                {
                    return Err(EditRefusal::ReservedName);
                }
                let replaced = match &save.previous_name {
                    Some(previous) => Some(position(previous).ok_or(EditRefusal::NotFound)?),
                    None => position(&save.server.name),
                };
                let kept = replaced.map(|index| &stored[index].env);
                let mut env = BTreeMap::new();
                for (name, value) in &save.env {
                    let value = match value {
                        Some(value) => value.clone(),
                        None => kept
                            .and_then(|kept| kept.get(name))
                            .cloned()
                            .ok_or_else(|| EditRefusal::EnvironmentValueMissing {
                                name: name.clone(),
                            })?,
                    };
                    if env.insert(name.clone(), value).is_some() {
                        return Err(EditRefusal::EnvironmentNameRepeated { name: name.clone() });
                    }
                }
                let saved = ConfiguredMcpServer {
                    server: save.server.clone(),
                    enabled: save.enabled,
                    env,
                };
                let mut edited = stored.to_vec();
                match replaced {
                    Some(index) => edited[index] = saved,
                    None => edited.push(saved),
                }
                Ok(edited)
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/configured_server.rs"]
mod tests;
