//! What the stored servers are launched as — the one place a
//! [`ConfiguredMcpServer`] becomes an [`McpServerLaunch`], at startup and after
//! each change — and the live set port over the SDK's [`McpServers`].
//!
//! ```text
//! ConfiguredMcpServer ──LaunchSettings::launch_set──▶ McpServerLaunch ──▶ McpServers::replace
//!                                                       └ McpServerLaunch::problem_in (the SDK's rules)
//! ```
use crate::mcp_servers::application::{LiveServerSet, LiveSetKept, ServerProblem};
use crate::mcp_servers::domain::{ConfiguredMcpServer, StdioServer, MANAGED_SERVER_NAME};
use nessa_sdk::infrastructure::{
    acp::sessions::{McpServerProblem, StdioMcpServer},
    mcp::{McpServerLaunch, McpServers},
};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

/// `server` as the SDK holds it.
pub fn sdk_server(server: &StdioServer) -> StdioMcpServer {
    StdioMcpServer {
        name: server.name.clone(),
        command: server.command.clone(),
        args: server.args.clone(),
    }
}

fn server(sdk: &StdioMcpServer) -> StdioServer {
    StdioServer {
        name: sdk.name.clone(),
        command: sdk.command.clone(),
        args: sdk.args.clone(),
    }
}

/// How this gateway launches a configured server, and the server Nessa
/// manages, as composition settled them.
#[derive(Clone, Debug)]
pub struct LaunchSettings {
    /// The managed server's launch ([`MANAGED_SERVER_NAME`]), when this
    /// gateway has one. It is not stored by any edit, and a stored entry
    /// under its name is left out in its favour.
    managed: Option<McpServerLaunch>,
    working_directory: PathBuf,
    /// The gateway's base environment for every server (`server_environment`).
    environment: BTreeMap<OsString, OsString>,
}

impl LaunchSettings {
    /// Settings that start each server in `working_directory` with
    /// `environment` beneath its own variables, and take the managed server
    /// from `configured` (the servers configured at startup).
    pub fn new(
        configured: &[ConfiguredMcpServer],
        working_directory: PathBuf,
        environment: BTreeMap<OsString, OsString>,
    ) -> Self {
        let mut settings = Self {
            managed: None,
            working_directory,
            environment,
        };
        settings.managed = configured
            .iter()
            .find(|server| server.managed() && server.enabled)
            .map(|server| settings.launch(server));
        settings
    }

    /// `server` as launched: in the working directory, with the base
    /// environment and its own variables over it — its value wins.
    fn launch(&self, server: &ConfiguredMcpServer) -> McpServerLaunch {
        let mut environment = self.environment.clone();
        for (name, value) in &server.env {
            environment.insert(name.into(), value.into());
        }
        McpServerLaunch {
            server: sdk_server(&server.server),
            working_directory: self.working_directory.clone(),
            environment,
        }
    }

    /// The live set for `stored`: the managed server, then each stored server
    /// that is on and not under the managed name.
    ///
    /// # Errors
    ///
    /// The first [`McpServerProblem`] of every stored server — on or off —
    /// with the managed one, as one set ([`McpServerLaunch::problem_in`]): a
    /// server turned off is still checked, and still counts.
    pub fn launch_set(
        &self,
        stored: &[ConfiguredMcpServer],
    ) -> Result<Vec<McpServerLaunch>, McpServerProblem> {
        let user = stored
            .iter()
            .filter(|server| server.server.name != MANAGED_SERVER_NAME);
        let every: Vec<McpServerLaunch> = self
            .managed
            .iter()
            .cloned()
            .chain(user.clone().map(|server| self.launch(server)))
            .collect();
        if let Some(problem) = McpServerLaunch::problem_in(&every) {
            return Err(problem);
        }
        Ok(self
            .managed
            .iter()
            .cloned()
            .chain(
                user.filter(|server| server.enabled)
                    .map(|server| self.launch(server)),
            )
            .collect())
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
    fn managed(&self) -> Option<StdioServer> {
        self.launches
            .managed
            .as_ref()
            .map(|launch| server(&launch.server))
    }

    fn problem(&self, stored: &[ConfiguredMcpServer]) -> Option<ServerProblem> {
        self.launches.launch_set(stored).err().map(problem)
    }

    fn replace(&self, stored: &[ConfiguredMcpServer]) -> Result<(), LiveSetKept> {
        let launches = self.launches.launch_set(stored).map_err(|_| LiveSetKept)?;
        self.servers.replace(launches).map_err(|_| LiveSetKept)
    }
}

/// The SDK's problem as the application names it.
fn problem(problem: McpServerProblem) -> ServerProblem {
    match problem {
        McpServerProblem::TooMany => ServerProblem::TooMany,
        McpServerProblem::DuplicateName { name } => ServerProblem::DuplicateName { name },
        McpServerProblem::Name => ServerProblem::Name,
        McpServerProblem::Command => ServerProblem::Command,
        McpServerProblem::Arguments => ServerProblem::Arguments,
        McpServerProblem::EnvironmentName => ServerProblem::EnvironmentName,
        McpServerProblem::ReservedEnvironmentName { name } => {
            ServerProblem::ReservedEnvironmentName { name }
        }
        McpServerProblem::EnvironmentValue { name } => ServerProblem::EnvironmentValue { name },
    }
}
