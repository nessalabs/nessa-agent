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
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The name of the server Nessa manages itself: the desktop's bundled one
/// (`composition::desktop`). No edit names it, and `mcpServers.list` marks it
/// managed.
pub const MANAGED_SERVER_NAME: &str = "nessa";

/// A stdio server: what the SDK's `StdioMcpServer` is, as this context holds
/// it (`infrastructure` converts between them). Its rules — the name, the
/// executable, the arguments — are the SDK's (`StdioMcpServer::problem`),
/// asked of the whole list through the live set port, so this constructor
/// checks none of them; its fields are read, never assigned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StdioServer {
    name: String,
    command: PathBuf,
    args: Vec<String>,
}
impl StdioServer {
    /// The server called `name`, started as `command` with `args`.
    pub fn new(name: impl Into<String>, command: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
        }
    }
    /// Its name, unique among the configured servers.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The executable it is started as.
    pub fn command(&self) -> &Path {
        &self.command
    }
    /// Its arguments, in order.
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

/// One server as `config.json` stores it under `agents.mcpServers`.
///
/// `Debug` names the variables in `env` and never prints a value, which may be
/// a credential (`a_configured_server_prints_its_variable_names_never_their_values`).
#[derive(Clone, PartialEq, Eq)]
pub struct ConfiguredMcpServer {
    server: StdioServer,
    enabled: bool,
    env: BTreeMap<String, String>,
}
impl ConfiguredMcpServer {
    /// `server`, in the launch set when `enabled`, given `env` — each
    /// variable's name and value, in any order.
    ///
    /// # Errors
    ///
    /// [`EnvironmentNameRepeated`] for the first name given twice, whatever
    /// the values: one map decoding the variables would keep only one of
    /// them, silently (`a_configured_server_refuses_a_repeated_variable_name`).
    pub fn new(
        server: StdioServer,
        enabled: bool,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, EnvironmentNameRepeated> {
        let env: Vec<(String, String)> = env.into_iter().collect();
        if let Some(name) = first_repeated(env.iter().map(|(name, _)| name.as_str())) {
            return Err(EnvironmentNameRepeated {
                name: name.to_owned(),
            });
        }
        Ok(Self {
            server,
            enabled,
            env: env.into_iter().collect(),
        })
    }
    /// The server: name, absolute executable, arguments.
    pub fn server(&self) -> &StdioServer {
        &self.server
    }
    /// Whether it is in the launch set. A server turned off stays stored.
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    /// The variables it is given over the gateway's own, by name; a value
    /// here wins.
    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }
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

/// A variable name given twice to one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentNameRepeated {
    /// The name.
    pub name: String,
}

/// The first of `names` that came before: the one rule that a server's
/// variables are named once, for a stored server and for a save.
fn first_repeated<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = std::collections::BTreeSet::new();
    names.into_iter().find(|name| !seen.insert(*name))
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
    /// [`EditRefusal`]: a reserved name, a name not stored, a variable given
    /// twice, or — the names once each — a variable kept that has no stored
    /// value.
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
                // A name given twice is said before a value missing for it.
                if let Some(name) = first_repeated(save.env.iter().map(|(name, _)| name.as_str())) {
                    return Err(EditRefusal::EnvironmentNameRepeated {
                        name: name.to_owned(),
                    });
                }
                // A stored value is kept only for the same launch: a save
                // that changes the command or arguments must give every value
                // again, so a new command cannot be pointed at a secret it
                // was never given (#480 adversarial review).
                let kept = replaced
                    .map(|index| &stored[index])
                    .filter(|stored| {
                        stored.server.command == save.server.command
                            && stored.server.args == save.server.args
                    })
                    .map(|stored| &stored.env);
                let env =
                    save.env
                        .iter()
                        .map(|(name, value)| {
                            let value = match value {
                                Some(value) => value.clone(),
                                None => kept.and_then(|kept| kept.get(name)).cloned().ok_or_else(
                                    || EditRefusal::EnvironmentValueMissing { name: name.clone() },
                                )?,
                            };
                            Ok((name.clone(), value))
                        })
                        .collect::<Result<Vec<_>, EditRefusal>>()?;
                let saved = ConfiguredMcpServer::new(save.server.clone(), save.enabled, env)
                    .map_err(|repeated| EditRefusal::EnvironmentNameRepeated {
                        name: repeated.name,
                    })?;
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
