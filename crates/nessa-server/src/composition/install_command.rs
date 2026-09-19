//! Construct the agent installer and report what it did.
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agent_install::application::{
    InstallAgentRuntime, InstallFailure, InstalledRuntime, SourceFailure, StoreFailure,
};
use crate::agent_install::domain::{AgentName, HostPlatform, PinnedRelease};
use crate::agent_install::infrastructure::{
    host_platform, releases_for, HttpsArchives, ManagedRuntimes,
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
pub(super) async fn execute(agent: &AgentName) -> Result<(), RunError> {
    let root = runtime_root()?;
    let agent = agent.clone();
    let installed = tokio::task::spawn_blocking(move || {
        install(&agent, &root).map(|installed| report(&agent, &installed))
    })
    .await
    .map_err(|error| RunError::Agent(format!("the installer did not finish: {error}")))??;
    write_report(&mut io::stdout().lock(), &installed)
}

/// The install itself, with every effect it needs constructed here.
fn install(agent: &AgentName, root: &Path) -> Result<InstalledRuntime, RunError> {
    let host = host_platform();
    let release = pinned(agent, &host)?;
    let source = HttpsArchives::new().map_err(|error| RunError::Agent(error.to_string()))?;
    let store = ManagedRuntimes::new(root);
    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
    }
    .execute(agent, &release, &host)
    .map_err(|failure| RunError::Agent(explain(&failure)))?;
    tracing::info!(
        agent = agent.as_str(),
        version = %installed.version,
        downloaded = installed.downloaded,
        "agent runtime installed"
    );
    Ok(installed)
}

/// The tested release for this agent on this machine.
///
/// The two ways there is none are two different things to be told. Claude and
/// Codex are expected to be on the machine already, so Nessa pins no release for
/// them at all and saying so by name beats a failure that reads as though a
/// download went wrong. An agent Nessa *does* install but not for this
/// platform is the opposite message: the agent is right, the machine is not one
/// there is a tested build for.
fn pinned(agent: &AgentName, host: &HostPlatform) -> Result<PinnedRelease, RunError> {
    let releases = releases_for(agent).map_err(|error| RunError::Agent(error.to_string()))?;
    if releases.is_empty() {
        return Err(RunError::Agent(format!(
            "{agent} is not an agent nessa installs"
        )));
    }
    preferred(releases, host)
        .ok_or_else(|| RunError::Agent(format!("nessa has no tested {agent} release for {host}")))
}

/// The best of `releases` for this machine, if any of them runs on it at all.
///
/// Its own function, taking the releases rather than reading them, because the
/// answer has to be the same whatever order the pin file happens to list them
/// in — and a test that asks this through the compiled-in file cannot tell a
/// preference from the first entry that matched.
fn preferred(releases: Vec<PinnedRelease>, host: &HostPlatform) -> Option<PinnedRelease> {
    releases
        .into_iter()
        .filter(|release| release.runs_on(host))
        // Several pinned archives can run here at once: the vendor publishes a
        // build that needs AVX2 and one that does not, and a machine with AVX2
        // runs either. Take the more demanding one, which is the vendor's own
        // default build — the undemanding one exists for machines that cannot
        // take it, and installing it everywhere would give up what it is there
        // to preserve.
        .max_by_key(|release| release.requirements().avx2())
}

/// Say what went wrong, and whether it is worth trying again.
///
/// Branched on the variant rather than passed through as one message, because
/// the variants are the whole reason the install use case reports typed
/// failures: an archive that was not the pinned one is the one case where
/// trying again is the wrong advice, and a person reading the last line of a
/// failed install is exactly who needs to be told which of the two they have.
fn explain(failure: &InstallFailure) -> String {
    match failure {
        // The platform is already in `{failure}`, so this adds what the person
        // does about it rather than saying the same thing twice.
        InstallFailure::UnsupportedPlatform(_) => format!(
            "{failure}; nothing was installed, and nothing will be until nessa \
             ships a tested build for this platform"
        ),
        InstallFailure::Download(SourceFailure::Refused(status)) => {
            format!("{failure}; the pinned release may have been withdrawn ({status})")
        }
        // A body that overran the bound will overrun it again, so this belongs
        // with the faults below rather than with the connection that dropped.
        InstallFailure::Download(SourceFailure::TooLarge(_))
        | InstallFailure::Rejected(_)
        | InstallFailure::Store(
            StoreFailure::MissingExecutable(_) | StoreFailure::MalformedArchive(_),
        ) => format!(
            "{failure}; nothing was installed and this is not worth retrying — \
             report it rather than running the command again"
        ),
        InstallFailure::Download(_) => format!("{failure}; nothing was installed, try again"),
        InstallFailure::Store(failure) => format!("{failure}; nothing was installed"),
    }
}

/// What the command says it did, on stdout, for whatever called it.
fn report(agent: &AgentName, installed: &InstalledRuntime) -> Value {
    json!({
        "agent": agent.as_str(),
        "version": installed.version.as_str(),
        "executable": installed.executable.to_string_lossy(),
        "downloaded": installed.downloaded,
    })
}

/// One JSON object, one line, so a caller can read it a line at a time.
fn write_report(out: &mut impl Write, report: &Value) -> Result<(), RunError> {
    serde_json::to_writer(&mut *out, report)
        .map_err(|_| RunError::Agent("install output failed".into()))?;
    writeln!(out).map_err(|_| RunError::Agent("install output failed".into()))
}

#[cfg(test)]
#[path = "../../tests/agent_install/install_command.rs"]
mod tests;
