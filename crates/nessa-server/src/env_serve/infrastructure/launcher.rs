//! How this host starts an agent's harness for a gateway's lease: what the
//! host's own `config.json` says for that agent, never what the gateway
//! sends.
//!
//! ```text
//! launch(agent, binding variables) ──▶ LaunchSpec for agent (host config)
//!     command = spec.executable spec.args, in workspace,
//!     environment = accepted binding variables, then the host's own (host wins),
//!                   then the lease's publish point (NESSA_ARTIFACTS), this side's,
//!                   and this build's directory first on the host's PATH
//!     ──▶ SupervisedHarness::start ──▶ HarnessProcess
//! ```
//!
//! Arrows are calls, in order. The gateway sets only the variables the
//! agent's binding declares it sets (`LAUNCH_VARIABLES`); any other name is
//! refused, so a gateway cannot choose the executable, the search path, a
//! preloaded library or a credential here. A harness given a publish point
//! finds `nessa` on its search path: the copy serving its lease, which a
//! gateway installs where no search path looks (`env_serve::install`).
use crate::env_serve::application::{HarnessLauncher, PUBLISH_POINT_VARIABLE};
use nessa_protocol::agents::AgentId;
use nessa_sdk::{
    application::agent_execution::{agents::AgentError, providers::HarnessProcess},
    infrastructure::{
        claude_acp::sessions::ClaudeAcpProvider, codex_acp::sessions::CodexAcpProvider,
        harness_process::SupervisedHarness,
    },
};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
};
use tokio::process::Command;

/// How the host starts one agent: its executable, arguments, and the
/// account variables and credentials it runs with.
#[derive(Clone, Debug)]
pub(crate) struct LaunchSpec {
    pub(crate) executable: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) environment: BTreeMap<OsString, OsString>,
}

/// The agents this host's configuration can start, in its workspace.
pub(crate) struct ConfiguredLauncher {
    workspace: String,
    agents: HashMap<AgentId, LaunchSpec>,
}

impl ConfiguredLauncher {
    /// `workspace` is absolute and UTF-8, which composition checks.
    pub(crate) fn new(workspace: String, agents: HashMap<AgentId, LaunchSpec>) -> Self {
        Self { workspace, agents }
    }
}

/// The variables `agent`'s binding sets for a launch: the only ones a gateway
/// may send. An agent with no binding that starts elsewhere has none, and is
/// never run here.
fn accepted(agent: AgentId) -> Option<&'static [&'static str]> {
    match agent {
        AgentId::Claude => Some(ClaudeAcpProvider::LAUNCH_VARIABLES),
        AgentId::Codex => Some(CodexAcpProvider::LAUNCH_VARIABLES),
        AgentId::Opencode => None,
    }
}

impl HarnessLauncher for ConfiguredLauncher {
    fn workspace(&self) -> &str {
        &self.workspace
    }

    fn runs(&self, agent: &str) -> bool {
        AgentId::parse(agent)
            .is_some_and(|agent| accepted(agent).is_some() && self.agents.contains_key(&agent))
    }

    fn launch(
        &self,
        agent: &str,
        environment: &BTreeMap<String, String>,
        publish_point: Option<&str>,
    ) -> Result<HarnessProcess, AgentError> {
        let id = AgentId::parse(agent)
            .ok_or_else(|| AgentError::Unsupported(format!("no agent named {agent}")))?;
        let (Some(names), Some(spec)) = (accepted(id), self.agents.get(&id)) else {
            return Err(AgentError::Unsupported(format!(
                "this host runs no {agent} harness"
            )));
        };
        if let Some(name) = environment
            .keys()
            .find(|name| !names.contains(&name.as_str()))
        {
            return Err(AgentError::InvalidInput(format!(
                "{name} is not a variable the {agent} binding sets"
            )));
        }
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .current_dir(&self.workspace)
            .env_clear()
            .envs(environment)
            .envs(&spec.environment);
        if let Some(point) = publish_point {
            command.env(PUBLISH_POINT_VARIABLE, point);
            let this_build = std::env::current_exe().ok();
            let configured = spec.environment.get(&OsString::from("PATH"));
            if let Some(path) = this_build
                .as_deref()
                .and_then(Path::parent)
                .and_then(|directory| search_path(directory, configured.map(OsString::as_os_str)))
            {
                command.env("PATH", path);
            }
        }
        SupervisedHarness::start(command)
    }
}

/// `directory` first, then the host's `configured` search path: what a
/// harness that may publish runs as `nessa` is the build serving it. `None`
/// when no search path can hold `directory`.
pub(crate) fn search_path(
    directory: &Path,
    configured: Option<&std::ffi::OsStr>,
) -> Option<OsString> {
    let rest = configured.map(std::env::split_paths).into_iter().flatten();
    std::env::join_paths(std::iter::once(directory.to_path_buf()).chain(rest)).ok()
}

#[cfg(test)]
#[path = "../../../tests/env_serve/launcher.rs"]
mod tests;
