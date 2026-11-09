//! How this host starts an agent's harness for a gateway's lease: what the
//! host's own `config.json` says for that agent, never what the gateway
//! sends.
//!
//! ```text
//! launch(agent, binding variables) ──▶ LaunchSpec for agent (host config)
//!     command = spec.executable spec.args, in workspace,
//!     environment = accepted binding variables, then the host's own,
//!                   then the gateway's saved login where the host has none
//!                   (host wins), then the lease's publish point (NESSA_ARTIFACTS), this side's,
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
    /// Where a saved Codex key's harness keeps native sessions. One directory
    /// for this host, so a later process can resume a thread the previous
    /// one stored. `None` hands no Codex login: a fresh directory each
    /// launch would.
    codex_sessions: Option<PathBuf>,
}

impl ConfiguredLauncher {
    /// Host-owned durable OpenCode sessions survive isolated harness homes.
    pub(crate) fn opencode_database(&mut self, path: PathBuf) {
        if let Some(spec) = self.agents.get_mut(&AgentId::Opencode) {
            spec.environment
                .insert("OPENCODE_DB".into(), path.into_os_string());
        }
    }
    /// The directory a saved Codex key uses as `CODEX_HOME`. Composition
    /// gives one path for this host; every launch reuses it.
    pub(crate) fn codex_sessions(&mut self, path: PathBuf) {
        self.codex_sessions = Some(path);
    }
    /// `workspace` is absolute and UTF-8, which composition checks.
    pub(crate) fn new(workspace: String, agents: HashMap<AgentId, LaunchSpec>) -> Self {
        Self {
            workspace,
            agents,
            codex_sessions: None,
        }
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
            .envs(environment)
            .envs(&spec.environment);
        // After the host's own environment, so a private Codex home is not
        // replaced by the host's `CODEX_HOME`. Host-wins already refused
        // this when the host has a sign-in.
        if let Some(handed) = login_to_set(
            id,
            login,
            &spec.environment,
            |path| path.exists(),
            self.codex_sessions.as_deref(),
        ) {
            for (name, value) in handed {
                command.env(name, value);
            }
        }
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

/// The ACP auth request the pinned Codex adapter (`codex-acp` 1.12.0)
/// runs when Codex requires authentication. `methodId` `api-key` reads
/// `CODEX_API_KEY`; the key is not copied into this JSON, which the
/// adapter logs at startup.
const CODEX_API_KEY_AUTH: &str = r#"{"methodId":"api-key"}"#;

/// Where the gateway's `login` for `agent` goes, only where the host has
/// no sign-in of its own for that agent. The host's own wins, whether it
/// is one of the agent's login variables in the `configured` environment
/// or a sign-in file the harness this launch starts will actually read
/// ([`own_sign_in_files`]), which `exists` is asked about. A sign-in kept
/// only in the macOS keychain is not seen.
///
/// Codex's app-server starts with environment-key authentication off, so
/// `CODEX_API_KEY` alone does not sign it in. A saved key is also the
/// adapter's `DEFAULT_AUTH_REQUEST` for the api-key method, and
/// `CODEX_HOME` is `codex_sessions`: one directory for this host, whose
/// `config.toml` sets `cli_auth_credentials_store` to `ephemeral`. Codex
/// 0.154 keeps that key in the process and its threads under
/// `sessions/` in that home, so the next harness can resume them. No
/// directory means no Codex login.
pub(crate) fn login_to_set(
    agent: AgentId,
    login: Option<&Login>,
    configured: &BTreeMap<OsString, OsString>,
    exists: impl Fn(&Path) -> bool,
    codex_sessions: Option<&Path>,
) -> Option<Vec<(String, String)>> {
    let variables = login_variables(agent);
    let host_has_own = variables
        .iter()
        // An exported but empty variable signs nothing in.
        .any(|name| {
            configured
                .get(&OsString::from(name))
                .is_some_and(|value| !value.is_empty())
        })
        || own_sign_in_files(agent, configured)
            .iter()
            .any(|path| exists(path));
    let login = match (login, host_has_own) {
        (Some(login), false) => login,
        _ => return None,
    };
    let variable = login_variable(agent, login);
    let mut handed = vec![(variable.to_owned(), login.expose().to_owned())];
    if agent == AgentId::Codex {
        let home = prepare_codex_home(codex_sessions?)?;
        handed.push((
            "DEFAULT_AUTH_REQUEST".to_owned(),
            CODEX_API_KEY_AUTH.to_owned(),
        ));
        handed.push(("CODEX_HOME".to_owned(), home));
    }
    Some(handed)
}

/// Codex 0.154's credential-store setting, and the value that keeps a
/// key in the process.
const CREDENTIAL_STORE: &str = "cli_auth_credentials_store";
const EPHEMERAL: &str = "ephemeral";

/// Make `directory` usable as `CODEX_HOME` for a saved key. The directory
/// stays: Codex writes thread rollouts under `sessions/` there, and a
/// later harness resumes by that id. `config.toml` keeps everything else
/// and sets the root credential-store key to ephemeral, so the key is not
/// written into `auth.json`. `None` when the directory cannot be prepared
/// or its config is not TOML: the key is then not handed.
fn prepare_codex_home(directory: &Path) -> Option<String> {
    std::fs::create_dir_all(directory).ok()?;
    let config = directory.join("config.toml");
    let existing = match std::fs::read_to_string(&config) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    // Set by TOML key, not by line: the root table's key however the file
    // spells it, placed before every table. A config that is not TOML is
    // left alone, and the key is not handed.
    let mut document: toml_edit::DocumentMut = existing.parse().ok()?;
    let ephemeral = document
        .get(CREDENTIAL_STORE)
        .and_then(toml_edit::Item::as_str)
        == Some(EPHEMERAL);
    if !ephemeral {
        document[CREDENTIAL_STORE] = toml_edit::value(EPHEMERAL);
        std::fs::write(&config, document.to_string()).ok()?;
    }
    directory.to_str().map(str::to_owned)
}

/// Where `agent` keeps a sign-in the harness this launch starts will
/// read, under the environment it is started with: Claude Code's
/// `.credentials.json` (in `CLAUDE_CONFIG_DIR`, else `~/.claude`) and
/// Codex's `auth.json` (in `CODEX_HOME`, else `~/.codex`). None where
/// neither directory is set.
///
/// OpenCode's `auth.json` is not one of these. A lease starts OpenCode
/// through [`OpencodeAcpProvider::start_on_host`], which gives the process
/// a private data directory so it cannot read the caller's file. The only
/// credential that process accepts is `OPENCODE_API_KEY` in the harness
/// environment, which [`login_variables`] already counts. Treating the
/// file as the host's sign-in would withhold the saved login and leave
/// the harness with neither.
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
        AgentId::Claude => under("CLAUDE_CONFIG_DIR", &[".claude"], ".credentials.json")
            .into_iter()
            .collect(),
        AgentId::Codex => under("CODEX_HOME", &[".codex"], "auth.json")
            .into_iter()
            .collect(),
        AgentId::Opencode => Vec::new(),
    }
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
