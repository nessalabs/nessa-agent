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
use uuid::Uuid;

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

/// A remote server as `config.json` stores it: a durable id, the name a
/// harness sees, and the endpoint. The URL's rules are the SDK's, asked of
/// the whole list through the live set; this constructor checks none of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteConfigured {
    id: Uuid,
    name: String,
    url: String,
    enabled: bool,
}

impl RemoteConfigured {
    /// `id`, shown as `name`, reached at `url`, in the live set when `enabled`.
    pub fn new(id: Uuid, name: impl Into<String>, url: impl Into<String>, enabled: bool) -> Self {
        Self {
            id,
            name: name.into(),
            url: url.into(),
            enabled,
        }
    }

    /// The id settings minted. Rename does not change it.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The name a harness sees.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The endpoint, as stored. Not yet checked.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Whether new openings are admitted.
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// One stored server, stdio or remote. The file is one list; this is that
/// list's element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredMcpServer {
    /// A local process.
    Stdio(ConfiguredMcpServer),
    /// An HTTP endpoint.
    Remote(RemoteConfigured),
}

impl StoredMcpServer {
    /// Its name, unique among the configured servers.
    pub fn name(&self) -> &str {
        match self {
            Self::Stdio(server) => server.server().name(),
            Self::Remote(server) => server.name(),
        }
    }

    /// Whether it is in the live set.
    pub fn enabled(&self) -> bool {
        match self {
            Self::Stdio(server) => server.enabled(),
            Self::Remote(server) => server.enabled(),
        }
    }

    /// Whether this is the server Nessa manages. A remote server is not.
    pub fn managed(&self) -> bool {
        match self {
            Self::Stdio(server) => server.managed(),
            Self::Remote(_) => false,
        }
    }

    /// The stdio server, when this is one.
    pub fn stdio(&self) -> Option<&ConfiguredMcpServer> {
        match self {
            Self::Stdio(server) => Some(server),
            Self::Remote(_) => None,
        }
    }

    /// The remote server, when this is one.
    pub fn remote(&self) -> Option<&RemoteConfigured> {
        match self {
            Self::Remote(server) => Some(server),
            Self::Stdio(_) => None,
        }
    }
}

impl From<ConfiguredMcpServer> for StoredMcpServer {
    fn from(server: ConfiguredMcpServer) -> Self {
        Self::Stdio(server)
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

/// A remote server as `mcpServers.save` asks for it. The id is the one
/// already stored when this replaces a remote entry; settings mints it when
/// the entry is new. The domain keeps the stored id on a remote replace, so
/// a caller cannot rotate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteServerSave {
    /// The name it is stored under now, when it is being renamed.
    pub previous_name: Option<String>,
    /// The id to use when this does not replace a remote entry.
    pub id: Uuid,
    /// The name it should be stored under.
    pub name: String,
    /// The endpoint, checked by the live set.
    pub url: String,
    /// Whether it should be in the live set.
    pub enabled: bool,
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
    /// Store a stdio server, adding it or replacing the one under its name
    /// (or under its previous name).
    Save(ServerSave),
    /// Store a remote server.
    SaveRemote(RemoteServerSave),
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
            Self::SaveRemote(save) => &save.name,
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
    pub fn apply(&self, stored: &[StoredMcpServer]) -> Result<Vec<StoredMcpServer>, EditRefusal> {
        let position = |name: &str| stored.iter().position(|each| each.name() == name);
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
            Self::SaveRemote(save) => {
                if save.name == MANAGED_SERVER_NAME
                    || save.previous_name.as_deref() == Some(MANAGED_SERVER_NAME)
                {
                    return Err(EditRefusal::ReservedName);
                }
                let replaced = match &save.previous_name {
                    Some(previous) => Some(position(previous).ok_or(EditRefusal::NotFound)?),
                    None => position(&save.name),
                };
                let id = replaced
                    .and_then(|index| stored[index].remote().map(RemoteConfigured::id))
                    .unwrap_or(save.id);
                let saved = StoredMcpServer::Remote(RemoteConfigured::new(
                    id,
                    save.name.clone(),
                    save.url.clone(),
                    save.enabled,
                ));
                let mut edited = stored.to_vec();
                match replaced {
                    Some(index) => edited[index] = saved,
                    None => edited.push(saved),
                }
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
                // A stored value is kept only for the same launch
                // ([`same_launch`]); otherwise every value is given again.
                let kept = replaced
                    .map(|index| &stored[index])
                    .filter(|stored| same_launch(stored, save))
                    .and_then(|stored| stored.stdio().map(ConfiguredMcpServer::env));
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
                let saved = StoredMcpServer::Stdio(
                    ConfiguredMcpServer::new(save.server.clone(), save.enabled, env).map_err(
                        |repeated| EditRefusal::EnvironmentNameRepeated {
                            name: repeated.name,
                        },
                    )?,
                );
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

/// Whether `save` launches `stored` exactly as it is launched now, apart from
/// the values it keeps: the same command and arguments, and the same
/// variables — none added, none left out — each kept (`None`) or given its
/// stored value. Only then is a kept value honoured. Anything else that
/// changes what the process runs or loads — a new executable, an argument, a
/// variable such as `LD_PRELOAD`, `NODE_OPTIONS` or `PYTHONPATH` — could hand
/// the secret to code it was never given to, and read it back through an
/// inspection (#480 adversarial review). The command is compared byte for
/// byte: `Path`'s equality reads `/bin//server` as `/bin/server`, a
/// different command as written.
fn same_launch(stored: &StoredMcpServer, save: &ServerSave) -> bool {
    let Some(stored) = stored.stdio() else {
        return false;
    };
    stored.server().command().as_os_str() == save.server.command.as_os_str()
        && stored.server().args() == save.server.args
        && save.env.len() == stored.env().len()
        && save
            .env
            .iter()
            .all(|(name, value)| match stored.env().get(name) {
                Some(stored) => value.as_ref().is_none_or(|value| value == stored),
                None => false,
            })
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/configured_server.rs"]
mod tests;
