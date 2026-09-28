//! Construct the agent installer and report what it did.
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use nessa_local_storage::create_directory;
use serde_json::{json, Value};

use crate::agent_install::application::{
    AgentInstallations, GatewayInstallFailure, InstallAgentRuntime, InstallFailure,
    InstallationOffer, InstalledRuntime, ReclamationWarning, RuntimeStateEvidence, RuntimeStore,
    SourceFailure, StoreFailure,
};
use crate::agent_install::domain::{
    preferred_release, AgentName, HostPlatform, InstallRequest, PinnedRelease, ReclamationEvent,
};
use crate::agent_install::infrastructure::{
    host_platform, releases_for, DurableInstallAudit, DurableInstallationDelivery,
    DurableReclamationAudit, HttpsArchives, ManagedRuntimes, UuidReclamationOperationIds,
};
use crate::agents::domain::AgentId;
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
    let request = InstallRequest::new(local_account_id(), uuid::Uuid::new_v4().to_string())
        .map_err(|error| RunError::Agent(error.to_string()))?;
    let installed = tokio::task::spawn_blocking(move || {
        install(&agent, &root, &request).and_then(|installed| report(&agent, &installed))
    })
    .await
    .map_err(|error| RunError::Agent(format!("the installer did not finish: {error}")))??;
    write_report(&mut io::stdout().lock(), &installed)
}

/// The install itself, with every effect it needs constructed here.
fn install(
    agent: &AgentName,
    root: &Path,
    request: &InstallRequest,
) -> Result<InstalledRuntime, RunError> {
    install_runtime(agent, root, request).map_err(|failure| match failure {
        GatewayInstallFailure::Install(failure) => RunError::Agent(explain(&failure)),
        GatewayInstallFailure::Unsupported => pinned(agent, &host_platform())
            .err()
            .unwrap_or_else(|| RunError::Agent("unsupported runtime".into())),
        GatewayInstallFailure::Unavailable => RunError::Agent("agent installer unavailable".into()),
    })
}

fn install_runtime(
    agent: &AgentName,
    root: &Path,
    request: &InstallRequest,
) -> Result<InstalledRuntime, GatewayInstallFailure> {
    let host = host_platform();
    let releases = releases_for(agent).map_err(installer_unavailable)?;
    let release = preferred_release(releases, &host).ok_or(GatewayInstallFailure::Unsupported)?;
    let data_root = root.parent().ok_or(GatewayInstallFailure::Unavailable)?;
    let audit = install_audit(data_root).map_err(installer_unavailable)?;
    let delivery = install_delivery(data_root).map_err(installer_unavailable)?;
    let reclamation_audit =
        DurableReclamationAudit::new(data_root.join("audit/agent-runtime-reclamation"))
            .map_err(installer_unavailable)?;
    let source = HttpsArchives::new().map_err(installer_unavailable)?;
    let store = ManagedRuntimes::new(root);
    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: &delivery,
        reclamation_audit: &reclamation_audit,
        reclamation_operation_ids: &UuidReclamationOperationIds,
    }
    .execute(agent, &release, &host, request)
    .map_err(|failure| GatewayInstallFailure::Install(Box::new(failure)))?;
    tracing::info!(
        agent = agent.as_str(),
        version = %installed.version,
        downloaded = installed.downloaded,
        "agent runtime installed"
    );
    Ok(installed)
}

/// Create the install journal beneath the selected private data namespace.
fn install_audit(data_root: &Path) -> Result<DurableInstallAudit, RunError> {
    create_directory(data_root).map_err(|error| RunError::Agent(error.to_string()))?;
    DurableInstallAudit::new(
        data_root,
        Path::new("audit/agent-install"),
        Arc::new(super::local_auth::SystemClock),
    )
    .map_err(|error| RunError::Agent(error.to_string()))
}

fn install_delivery(data_root: &Path) -> Result<DurableInstallationDelivery, RunError> {
    create_directory(data_root).map_err(|error| RunError::Agent(error.to_string()))?;
    DurableInstallationDelivery::new(
        data_root,
        Path::new("installation-delivery/agent-install"),
        Arc::new(super::local_auth::SystemClock),
    )
    .map_err(|error| RunError::Agent(error.to_string()))
}

/// The tested release for this agent on this machine.
///
/// The two ways there is none are two different things to be told: an agent
/// this build pins nothing for at all, and one it pins for other machines but
/// not this one. The first is the agent being wrong, the second is the machine
/// being wrong, and a single failure would read as a download that went astray.
///
fn pinned(agent: &AgentName, host: &HostPlatform) -> Result<PinnedRelease, RunError> {
    let releases = releases_for(agent).map_err(|error| RunError::Agent(error.to_string()))?;
    if releases.is_empty() {
        return Err(RunError::Agent(format!(
            "{agent} is not an agent nessa installs"
        )));
    }
    preferred_release(releases.clone(), host)
        .ok_or_else(|| RunError::Agent(unrunnable(agent, host, &releases)))
}

/// Say why none of the pinned builds runs here, in terms the reader can act
/// on.
///
/// "No tested release for linux-x86_64" is a true sentence and a useless one
/// when linux-x86_64 is pinned four times over: what is missing is not the
/// platform, it is something about *this machine* the builds all ask for and
/// it does not provide. The likeliest case is a machine whose C library could
/// not be established, and the reader has no way to guess that from the
/// platform alone. So the builds that exist for the platform are listed by
/// what each of them needs, which is the difference between "nessa does not
/// support my computer" and "nessa could not tell which of these fits".
fn unrunnable(agent: &AgentName, host: &HostPlatform, releases: &[PinnedRelease]) -> String {
    let mut needs: Vec<String> = releases
        .iter()
        .filter(|release| release.platform() == host.platform())
        .map(|release| release.requirements().to_string())
        .collect();
    needs.dedup();
    if needs.is_empty() {
        return format!("nessa has no tested {agent} release for {host}");
    }
    format!(
        "nessa has no tested {agent} release this machine can run: it is {host}, \
         and the {agent} builds for {} need {}",
        host.platform(),
        needs.join(", or ")
    )
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
             ships a build this machine can run"
        ),
        InstallFailure::Download(SourceFailure::Refused(status)) => {
            format!("{failure}; the pinned release may have been withdrawn ({status})")
        }
        // A body that overran the bound will overrun it again, so this belongs
        // with the faults below rather than with the connection that dropped.
        InstallFailure::Download(SourceFailure::TooLarge(_))
        | InstallFailure::Rejected(_)
        | InstallFailure::Store(
            StoreFailure::IncompleteArchive(_) | StoreFailure::MalformedArchive(_),
        ) => format!(
            "{failure}; nothing was installed and this is not worth retrying — \
             report it rather than running the command again"
        ),
        InstallFailure::Download(_) => format!("{failure}; nothing was installed, try again"),
        InstallFailure::Store(failure) => format!("{failure}; nothing was installed"),
        InstallFailure::Recovery { .. } => failure.to_string(),
        InstallFailure::Evidence(_) => format!("{failure}; the install result was not reported"),
        InstallFailure::AttemptReused(_) => {
            format!("{failure}; retry retained audit evidence or begin a new invocation")
        }
        InstallFailure::Audit(evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::Unchanged =>
        {
            format!("{failure}; no unaudited runtime was reported as installed")
        }
        InstallFailure::Audit(_) => failure.to_string(),
        InstallFailure::Delivery(_) => format!(
            "{failure}; publication evidence is uncertain, and new installs for this account are \
             blocked until recovery succeeds"
        ),
        InstallFailure::UnresolvedPublication(_) => format!(
            "{failure}; the prior publication result is unknown, and new installs for this account \
            are blocked until its exact outcome is recovered"
        ),
        InstallFailure::Reclamation { .. } => format!(
            "{failure}; the replacement is installed, but publication remains unsettled until its cleanup obligation is retained"
        ),
        InstallFailure::ReclamationRecovery(_) => format!(
            "{failure}; publication and new installs are blocked until the exact replacement cleanup evidence is reconciled"
        ),
    }
}

#[cfg(unix)]
pub(super) fn local_account_id() -> String {
    // The process runs with the account whose private data directory receives
    // the runtime and audit records. The effective uid is the verified OS
    // identity behind those permissions; no human name is guessed from it.
    format!("unix:{}", unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
pub(super) fn local_account_id() -> String {
    // No runtime is pinned for a non-Unix host. Kept explicit so an eventual
    // Windows pin cannot silently claim a human identity it has not verified.
    "unsupported-process-account".to_owned()
}

/// What the command says it did, on stdout, for whatever called it.
///
/// Fallible for one reason: a path is bytes on Unix, and the data directory
/// this one is built under comes from `NESSA_DATA_DIR` or the home directory,
/// neither of which has to be valid UTF-8. `to_string_lossy` would turn every
/// undecodable byte into U+FFFD and report a path that names nothing — a
/// successful install whose one machine-readable field points at a file that
/// does not exist, which a caller would then fail to launch with no idea why.
///
/// So it refuses instead, and the refusal says what is true: the runtime is
/// installed, and only the report cannot be written. Installing again is free
/// once the directory has a name that can be spelled, because an install that
/// is already there downloads nothing.
fn report(agent: &AgentName, installed: &InstalledRuntime) -> Result<Value, RunError> {
    let executable = installed.executable.to_str().ok_or_else(|| {
        RunError::Agent(format!(
            "{agent} is installed at {}, which is not a path this command can report as text; \
             nothing else is wrong with the installation",
            installed.executable.display()
        ))
    })?;
    Ok(json!({
        "agent": agent.as_str(),
        "version": installed.version.as_str(),
        "executable": executable,
        "downloaded": installed.downloaded,
        "reclamationWarnings": installed
            .reclamation_warnings
            .iter()
            .map(reclamation_warning)
            .collect::<Vec<_>>(),
    }))
}

fn reclamation_warning(warning: &ReclamationWarning) -> Value {
    match warning {
        ReclamationWarning::Persistence(failure) => json!({
            "kind": "persistence",
            "stage": format!("{:?}", failure.stage()),
            "detail": failure.detail(),
        }),
        ReclamationWarning::OutcomePersistence { event, failure } => {
            reclamation_event("outcome_persistence", event, Some(failure.detail()))
        }
        ReclamationWarning::Outcome(event) => reclamation_event("outcome", event, None),
        ReclamationWarning::Audit { event, failure } => {
            reclamation_event("audit", event, Some(failure.detail()))
        }
        ReclamationWarning::AuditLookup {
            operation_id,
            failure,
        } => json!({
            "kind": "audit_lookup",
            "operationId": operation_id.as_str(),
            "detail": failure.detail(),
        }),
    }
}

fn reclamation_event(kind: &str, event: &ReclamationEvent, detail: Option<&str>) -> Value {
    let admission = event.admission();
    json!({
        "kind": kind,
        "operationId": admission.operation_id().as_str(),
        "agent": admission.agent().as_str(),
        "supersededVersion": admission.obligation().superseded().version().as_str(),
        "supersededDigest": admission.obligation().superseded().digest().as_str(),
        "currentVersion": admission.current().version().as_str(),
        "currentDigest": admission.current().digest().as_str(),
        "cause": format!("{:?}", admission.trigger().cause()),
        "initiator": admission.trigger().caller().map(|request| json!({
            "accountId": request.account_id(),
            "requestId": request.request_id(),
        })),
        "outcome": format!("{:?}", event.outcome()),
        "detail": detail,
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

/// Gateway composition shares the CLI's installer and exact-pin policy.
pub(super) struct GatewayInstaller {
    pub root: PathBuf,
    pub account_id: String,
}
impl AgentInstallations for GatewayInstaller {
    fn account_id(&self) -> &str {
        &self.account_id
    }
    fn offers(&self) -> Result<Vec<InstallationOffer>, GatewayInstallFailure> {
        let host = host_platform();
        let store = ManagedRuntimes::new(&self.root);
        let mut offers = Vec::new();
        for id in AgentId::ALL {
            let agent = AgentName::parse(id.name()).map_err(installer_unavailable)?;
            let releases = releases_for(&agent).map_err(installer_unavailable)?;
            if let Some(release) = preferred_release(releases, &host) {
                let installed = store
                    .installed(&agent, &release)
                    .map_err(installer_unavailable)?
                    .is_some();
                offers.push(InstallationOffer {
                    agent,
                    version: release.version().as_str().into(),
                    archive_bytes: release.archive_size().bytes(),
                    installed,
                });
            }
        }
        Ok(offers)
    }
    fn install(
        &self,
        agent: &AgentName,
        request: &InstallRequest,
    ) -> Result<InstalledRuntime, GatewayInstallFailure> {
        install_runtime(agent, &self.root, request)
    }
}

fn installer_unavailable(error: impl std::fmt::Display) -> GatewayInstallFailure {
    tracing::warn!(%error, "agent installer could not access a required dependency");
    GatewayInstallFailure::Unavailable
}
