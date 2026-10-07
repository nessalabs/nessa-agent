//! What the stored servers are launched as — the one place a
//! [`ConfiguredMcpServer`] becomes an [`McpServerLaunch`], at startup, after
//! each change, and for an inspection — and the live set port over the SDK's
//! [`McpServers`].
//!
//! ```text
//! ConfiguredMcpServer ──LaunchSettings::launch_set──▶ McpServerLaunch ──▶ McpServers::replace
//!                                                       └ McpServerLaunch::problem_in (the SDK's rules)
//! ```
//!
//! The managed server (`nessa`) is the one the gateway started with: the
//! desktop's bundled one, or — on a gateway without the desktop — the one
//! stored under that name at startup, turned on or off. It is checked and
//! counted with the stored servers whether on or off, as a stored server is,
//! and launched only when on; no edit names it, and a stored entry under its
//! name is never launched in its place
//! (`a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off`).
//! On the desktop the managed server is bundled ([`LiveServerSet::bundled`]),
//! so a stored entry under its name is never used, and the next change drops
//! it from the file
//! (`a_desktop_change_drops_a_stored_nessa_and_its_audit_says_so`).
use crate::mcp_servers::application::{LiveServerSet, LiveSetKept, ServerProblem};
use crate::mcp_servers::domain::{
    ConfiguredMcpServer, StdioServer, StoredMcpServer, MANAGED_SERVER_NAME,
};
use nessa_sdk::infrastructure::{
    acp::sessions::{McpServerProblem, StdioMcpServer},
    mcp::{configuration_problem, McpServerLaunch, McpServers, RemoteMcpServer},
};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

/// `server` as the SDK holds it.
pub fn sdk_server(server: &StdioServer) -> StdioMcpServer {
    StdioMcpServer {
        name: server.name().to_owned(),
        command: server.command().to_owned(),
        args: server.args().to_vec(),
    }
}

/// How this gateway launches a configured server, and the server Nessa
/// manages, as composition settled them.
///
/// `Debug` names the managed server and the base environment's variables,
/// never a value (`launch_settings_print_names_never_values`).
#[derive(Clone)]
pub struct LaunchSettings {
    /// The managed server ([`MANAGED_SERVER_NAME`]) as the gateway started
    /// with it, on or off, when it has one. It is not stored by any edit,
    /// and a stored entry under its name is left out in its favour.
    managed: Option<ConfiguredMcpServer>,
    /// Whether the managed server is the desktop's bundled one rather than
    /// the one stored under its name.
    bundled: bool,
    working_directory: PathBuf,
    /// The gateway's base environment for every server (`server_environment`).
    environment: BTreeMap<OsString, OsString>,
}

impl std::fmt::Debug for LaunchSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchSettings")
            .field(
                "managed",
                &self.managed.as_ref().map(|managed| managed.server().name()),
            )
            .field("bundled", &self.bundled)
            .field("working_directory", &self.working_directory)
            .field("environment", &self.environment.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl LaunchSettings {
    /// Settings that start each server in `working_directory` with
    /// `environment` beneath its own variables, and take the managed server
    /// from `configured` (the servers configured at startup) — the desktop's
    /// bundled one when `bundled`, which composition put there in place of
    /// any stored under its name (`composition::desktop::configure`).
    pub fn new(
        configured: &[StoredMcpServer],
        bundled: bool,
        working_directory: PathBuf,
        environment: BTreeMap<OsString, OsString>,
    ) -> Self {
        Self {
            managed: configured
                .iter()
                .find_map(|server| server.managed().then(|| server.stdio().cloned()).flatten()),
            bundled,
            working_directory,
            environment,
        }
    }

    /// `server` as launched: in the working directory, with the base
    /// environment and its own variables over it — its value wins. What the
    /// live set and an inspection both start.
    pub(super) fn launch(&self, server: &ConfiguredMcpServer) -> McpServerLaunch {
        let mut environment = self.environment.clone();
        for (name, value) in server.env() {
            environment.insert(name.into(), value.into());
        }
        McpServerLaunch {
            server: sdk_server(server.server()),
            working_directory: self.working_directory.clone(),
            environment,
        }
    }

    /// The live set for `stored`: the managed server, then each stored server
    /// not under the managed name — those that are on.
    ///
    /// # Errors
    ///
    /// The first [`McpServerProblem`] of every stored server — on or off —
    /// with the managed one, on or off, as one set
    /// ([`McpServerLaunch::problem_in`]): a server turned off is still
    /// checked, and still counts.
    /// The live stdio launches and remote servers for `stored`.
    ///
    /// # Errors
    ///
    /// The first problem of the combined set, including servers that are off.
    pub fn launch_set(
        &self,
        stored: &[StoredMcpServer],
    ) -> Result<(Vec<McpServerLaunch>, Vec<RemoteMcpServer>), McpServerProblem> {
        let mut stdio = Vec::new();
        let mut remotes = Vec::new();
        if let Some(managed) = &self.managed {
            stdio.push(managed.clone());
        }
        for server in stored {
            if server.name() == MANAGED_SERVER_NAME {
                continue;
            }
            match server {
                StoredMcpServer::Stdio(server) => stdio.push(server.clone()),
                StoredMcpServer::Remote(remote) => remotes.push(remote.clone()),
            }
        }
        let launches: Vec<McpServerLaunch> =
            stdio.iter().map(|server| self.launch(server)).collect();
        let remote_servers: Vec<RemoteMcpServer> = remotes
            .iter()
            .map(|remote| RemoteMcpServer::new(remote.id(), remote.name(), remote.url()))
            .collect::<Result<_, _>>()
            .map_err(|error| match error {
                nessa_sdk::infrastructure::mcp::McpError::InvalidConfiguration(problem) => problem,
                _ => McpServerProblem::Url {
                    server: "remote".into(),
                },
            })?;
        if let Some(problem) = configuration_problem(&launches, &remote_servers) {
            return Err(problem);
        }
        let launches = stdio
            .iter()
            .zip(launches)
            .filter(|(server, _)| server.enabled())
            .map(|(_, launch)| launch)
            .collect();
        let remotes = remotes
            .into_iter()
            .zip(remote_servers)
            .filter(|(stored, _)| stored.enabled())
            .map(|(_, remote)| remote)
            .collect();
        Ok((launches, remotes))
    }
}

/// The SDK's live set, replaced with what [`LaunchSettings`] makes of the
/// stored servers.
pub struct LiveMcpServers {
    servers: McpServers,
    launches: LaunchSettings,
}

impl LiveMcpServers {
    pub fn new(servers: McpServers, launches: LaunchSettings) -> Self {
        Self { servers, launches }
    }
}

impl LiveServerSet for LiveMcpServers {
    fn managed(&self) -> Option<ConfiguredMcpServer> {
        self.launches.managed.clone()
    }

    fn bundled(&self) -> bool {
        self.launches.bundled
    }

    fn problem(&self, stored: &[StoredMcpServer]) -> Option<ServerProblem> {
        self.launches.launch_set(stored).err().map(problem)
    }

    fn replace(&self, stored: &[StoredMcpServer]) -> Result<(), LiveSetKept> {
        let (launches, remotes) = self.launches.launch_set(stored).map_err(|_| LiveSetKept)?;
        self.servers
            .replace_all(launches, remotes)
            .map_err(|_| LiveSetKept)
    }

    /// The live set now, less `name`: a subset of a set the SDK took, so
    /// only a stop refuses it. The store's lock is held, so nothing
    /// replaces the set between the read and the replacement. A name not
    /// in it replaces nothing.
    fn withdraw(&self, name: &str) -> Result<bool, LiveSetKept> {
        let launches = self.servers.configured();
        let remotes = self.servers.configured_remotes();
        let next_launches: Vec<_> = launches
            .iter()
            .filter(|launch| launch.server.name != name)
            .cloned()
            .collect();
        let next_remotes: Vec<_> = remotes
            .iter()
            .filter(|remote| remote.name() != name)
            .cloned()
            .collect();
        if next_launches.len() == launches.len() && next_remotes.len() == remotes.len() {
            return Ok(false);
        }
        self.servers
            .replace_all(next_launches, next_remotes)
            .map_err(|_| LiveSetKept)?;
        Ok(true)
    }
}

/// The SDK's problem as the application names it.
pub(super) fn problem(problem: McpServerProblem) -> ServerProblem {
    match problem {
        McpServerProblem::TooMany => ServerProblem::TooMany,
        McpServerProblem::DuplicateName { server } => ServerProblem::DuplicateName { server },
        McpServerProblem::Name { server } => ServerProblem::Name { server },
        McpServerProblem::Command { server } => ServerProblem::Command { server },
        McpServerProblem::Arguments { server } => ServerProblem::Arguments { server },
        McpServerProblem::EnvironmentName { server, name } => {
            ServerProblem::EnvironmentName { server, name }
        }
        McpServerProblem::ReservedEnvironmentName { server, name } => {
            ServerProblem::ReservedEnvironmentName { server, name }
        }
        McpServerProblem::EnvironmentValue { server, name } => {
            ServerProblem::EnvironmentValue { server, name }
        }
        McpServerProblem::Url { server } => ServerProblem::Url { server },
        McpServerProblem::DuplicateServerId { server } => {
            ServerProblem::DuplicateServerId { server }
        }
    }
}
