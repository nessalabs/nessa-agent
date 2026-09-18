//! Construct the agent installer and report what it did.
use std::io::{self, Write};
use std::path::PathBuf;

use serde_json::json;

use crate::agent_install::application::InstallAgentRuntime;
use crate::agent_install::infrastructure::{
    host_platform, release_for, HttpsArchives, ManagedRuntimes,
};
use crate::core::RunError;
use crate::env::Environment;

/// Where installed agent runtimes live, beside the data this stage already owns.
///
/// Derived from the auth directory's parent for the same reason
/// [`super::desktop`] does: that is the one path the environment resolves, and
/// a runtime installed for one stage or instance must not be picked up by
/// another.
fn runtime_root() -> Result<PathBuf, RunError> {
    let auth = Environment::auth_directory_from_system()?;
    let root = auth
        .parent()
        .ok_or_else(|| RunError::Agent("invalid data directory".into()))?;
    Ok(root.join("agents"))
}

/// `nessa install-agent NAME`.
///
/// Runs on a blocking thread rather than on the async runtime this process
/// starts with. The work is a large download, a hash and an unpack — all
/// blocking, none of it async — and the HTTP client it uses declines to run
/// inside a runtime at all. `spawn_blocking` is what keeps both of those true
/// without the command having to be reached before the runtime exists.
pub(super) async fn execute(agent: &str) -> Result<(), RunError> {
    let root = runtime_root()?;
    let agent = agent.to_owned();
    let installed = tokio::task::spawn_blocking(move || install(&agent, &root))
        .await
        .map_err(|error| RunError::Agent(format!("the installer did not finish: {error}")))??;
    serde_json::to_writer(io::stdout().lock(), &installed)
        .map_err(|_| RunError::Agent("install output failed".into()))?;
    writeln!(io::stdout()).map_err(|_| RunError::Agent("install output failed".into()))?;
    Ok(())
}

/// The install itself, with every effect it needs constructed here.
fn install(agent: &str, root: &std::path::Path) -> Result<serde_json::Value, RunError> {
    let platform = host_platform();
    let release = release_for(agent, &platform)
        .map_err(|error| RunError::Agent(error.to_string()))?
        .ok_or_else(|| {
            // Not every agent is one Nessa installs: Claude and Codex are
            // expected to be there already. Saying so by name beats a failure
            // that reads as though the download went wrong.
            RunError::Agent(format!(
                "nessa has no tested {agent} release for {platform}; it is not an agent nessa installs"
            ))
        })?;
    let source = HttpsArchives::new().map_err(|error| RunError::Agent(error.to_string()))?;
    let store = ManagedRuntimes::new(root);
    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(agent, &release, &platform)
    .map_err(|error| RunError::Agent(error.to_string()))?;
    tracing::info!(
        agent,
        version = %installed.version,
        downloaded = installed.downloaded,
        "agent runtime installed"
    );
    Ok(json!({
        "agent": agent,
        "version": installed.version.as_str(),
        "executable": installed.executable.to_string_lossy(),
        "downloaded": installed.downloaded,
    }))
}

#[cfg(test)]
#[path = "../../tests/agent_install/install_command.rs"]
mod tests;
