//! How this host starts an agent's harness for a gateway's lease: what the
//! host's own `config.json` says for that agent, never what the gateway
//! sends.
//!
//! ```text
//! launch(agent, binding variables) ──▶ LaunchSpec for agent (host config)
//!     command = spec.executable spec.args, in workspace,
//!     environment = accepted binding variables, the gateway's saved login
//!                   in the agent's login variable, then the host's own (host wins),
//!                   then the lease's publish point (NESSA_ARTIFACTS), this side's,
//!                   and this build's directory first on the host's PATH
//!     ──▶ SupervisedHarness::start ──▶ HarnessProcess
//! ```
//!
//! Arrows are calls, in order. The gateway sets only the variables the
//! agent's binding declares it sets (`LAUNCH_VARIABLES`); any other name is
//! refused, so a gateway cannot choose the executable, the search path, a
//! preloaded library or a credential here. The one credential it may hand
//! over is the agent's own saved login, only under a lease this host's
//! grant lets take one, and only in the variable the agent reads its login
//! from (`lease::login_variable`); a login the host
//! configures itself wins. A harness given a publish point finds `nessa` on its search path: the copy serving its lease, which a
//! gateway installs where no search path looks (`env_serve::install`).
use crate::env_serve::application::{HarnessLauncher, PUBLISH_POINT_VARIABLE};
use nessa_protocol::{
    agents::AgentId,
    lease::{login_variable, login_variables, Login},
};
use nessa_sdk::{
    application::agent_execution::{agents::AgentError, providers::HarnessProcess},
    domain::agent_execution::leases::SandboxProfiles,
    infrastructure::{
        claude_acp::sessions::ClaudeAcpProvider, codex_acp::sessions::CodexAcpProvider,
        harness_process::SupervisedHarness, opencode_acp::sessions::OpencodeAcpProvider,
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
    /// Host-owned durable OpenCode sessions survive isolated harness homes.
    pub(crate) fn opencode_database(&mut self, path: PathBuf) {
        if let Some(spec) = self.agents.get_mut(&AgentId::Opencode) {
            spec.environment
                .insert("OPENCODE_DB".into(), path.into_os_string());
        }
    }
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
        AgentId::Opencode => Some(OpencodeAcpProvider::LAUNCH_VARIABLES),
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

    /// Only the harness's own default: the host starts it as configured and
    /// sets nothing that would enclose it more.
    fn sandboxes(&self) -> SandboxProfiles {
        SandboxProfiles::HARNESS_DEFAULT
    }

    fn launch(
        &self,
        agent: &str,
        environment: &BTreeMap<String, String>,
        login: Option<&Login>,
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
            .envs(environment);
        if let Some((variable, login)) =
            login_to_set(id, login, &spec.environment, |path| path.exists())
        {
            command.env(variable, login);
        }
        command.envs(&spec.environment);
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
        if id == AgentId::Opencode {
            OpencodeAcpProvider::start_on_host(command)
        } else {
            SupervisedHarness::start(command)
        }
    }
}

/// Where the gateway's `login` for `agent` goes, and its value: the
/// variable the agent reads its login from, only where the host has no
/// sign-in of its own for that agent. The host's own wins, whether it is
/// one of the agent's login variables in the `configured` environment or
/// the agent's own sign-in file where that environment points it
/// ([`own_sign_in_files`]), which `exists` is asked about. A sign-in kept
/// only in the macOS keychain is not seen.
pub(crate) fn login_to_set<'a>(
    agent: AgentId,
    login: Option<&'a Login>,
    configured: &BTreeMap<OsString, OsString>,
    exists: impl Fn(&Path) -> bool,
) -> Option<(&'static str, &'a str)> {
    let variables = login_variables(agent);
    let host_has_own = variables
        .iter()
        .any(|name| configured.contains_key(&OsString::from(name)))
        || own_sign_in_files(agent, configured)
            .iter()
            .any(|path| exists(path));
    match (login, host_has_own) {
        (Some(login), false) => Some((login_variable(agent, login), login.expose())),
        _ => None,
    }
}

/// Where `agent` keeps a sign-in its own login command wrote, under the
/// environment the harness is started with: Claude Code's
/// `.credentials.json` (in `CLAUDE_CONFIG_DIR`, else `~/.claude`), Codex's
/// `auth.json` (in `CODEX_HOME`, else `~/.codex`), and OpenCode's
/// `auth.json` (in `$XDG_DATA_HOME/opencode`, else
/// `~/.local/share/opencode`). None where neither is set.
pub(crate) fn own_sign_in_files(
    agent: AgentId,
    configured: &BTreeMap<OsString, OsString>,
) -> Vec<std::path::PathBuf> {
    let get = |name: &str| {
        configured
            .get(&OsString::from(name))
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
    };
    let home = get("HOME");
    let under = |variable: &str, default: &[&str], file: &str| {
        get(variable)
            .or_else(|| {
                home.as_ref().map(|home| {
                    default
                        .iter()
                        .fold(home.clone(), |path, part| path.join(part))
                })
            })
            .map(|directory| directory.join(file))
    };
    match agent {
        AgentId::Claude => under("CLAUDE_CONFIG_DIR", &[".claude"], ".credentials.json"),
        AgentId::Codex => under("CODEX_HOME", &[".codex"], "auth.json"),
        AgentId::Opencode => get("XDG_DATA_HOME")
            .map(|data| data.join("opencode"))
            .or_else(|| home.as_ref().map(|home| home.join(".local/share/opencode")))
            .map(|directory| directory.join("auth.json")),
    }
    .into_iter()
    .collect()
}

/// What `execvp` searches when `PATH` is unset.
const UNSET_PATH: &str = "/bin:/usr/bin";

/// `directory` first, then the host's `configured` search path: what a
/// harness that may publish runs as `nessa` is the build serving it. With
/// none configured, the rest is what a lookup with no `PATH` searches, so
/// adding `directory` never takes a command away. `None` when no search
/// path can hold `directory`.
pub(crate) fn search_path(
    directory: &Path,
    configured: Option<&std::ffi::OsStr>,
) -> Option<OsString> {
    let configured = configured.unwrap_or(std::ffi::OsStr::new(UNSET_PATH));
    let rest = std::env::split_paths(configured);
    std::env::join_paths(std::iter::once(directory.to_path_buf()).chain(rest)).ok()
}

#[cfg(test)]
#[path = "../../../tests/env_serve/launcher.rs"]
mod tests;
