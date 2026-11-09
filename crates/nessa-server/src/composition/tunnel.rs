//! The web tunnel, composed with native pairing when `config.json` turns it
//! on (`docs/design/auth/tunnel.md`): the setting read with its token, the
//! loopback ingress bound with the native listener, and the provider's
//! program kept up beside it.
//!
//! ```text
//! prepare: native.tunnel + <namespace>/tunnel/token ──▶ TunnelSetting, status Starting
//!          (a named tunnel whose token cannot be read: Failed("token"), nothing runs)
//! bind:    the ingress address (loopback) ──▶ TunnelIngress, beside the native listener
//! start:   TunnelIngress::serve
//!          shepherd ServiceSupervisor(CloudflareService, kept in <tunnel>/kept)
//!            ──▶ its status, a failure by the step that failed (a refused launch by
//!                the reason CloudflareService recorded, LaunchRefusal)
//!            ──▶ TunnelStatus ──▶ pairing.tunnel, server.reach
//! stop:    both asked to stop; the provider's program is left running
//!          (ServiceStop::Leave) for the next run to take back when its spec
//!          and revision still match
//! off:     a program a last run left running is ended (release)
//! ```
//!
//! Arrows are construction and the order things start. The ingress forwards
//! into the native listener, whose tunnel edge admits only paired keys
//! (`device_pairing::infrastructure::connection`).
use super::runtime_config::TunnelConfig;
use crate::agent_install::application::ArchiveSource;
use crate::agent_install::infrastructure::HttpsArchives;
use crate::tunnel::application::{
    CloudflareTunnel, LaunchRefusal, TunnelFailure, TunnelMode, TunnelProvider, TunnelSetting,
    TunnelStatus, TunnelView,
};
use crate::tunnel::infrastructure::cloudflare::{
    CloudflareService, MakeArchives, Program, ShepherdKept, READY_WITHIN,
};
use crate::tunnel::infrastructure::files::TunnelFiles;
use shepherd::{
    KeptError, KeptName, KeptProcesses, ServiceFailureKind, ServiceOptions, ServiceStatus,
    ServiceStop, SupervisorBuilder, TerminateOptions,
};
use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, PoisonError},
};
use tokio::{sync::watch, task::JoinHandle};

/// The provider's program, by the name shepherd keeps it under.
const SERVICE_NAME: &str = "cloudflared";
/// Who the kept records say started it.
const OWNER: &str = "nessa-gateway";
/// The record the gateway's own kept-process host wrote before shepherd
/// kept the program (#1147).
const UNKEPT_RECORD: &str = "process.json";

/// The tunnel as this run serves it, before anything is bound.
pub(super) struct PreparedTunnel {
    files: TunnelFiles,
    ingress: SocketAddr,
    /// `None` when the setting could not be completed (a named tunnel's
    /// token unreadable): the status says why, and nothing runs.
    setting: Option<TunnelSetting>,
    status: watch::Sender<TunnelStatus>,
    /// The setting this run serves, as Settings compares a change with
    /// it: emptied when the ingress cannot be bound, so the same setting
    /// saved again is tried again.
    running: watch::Sender<Option<TunnelSetting>>,
}

impl PreparedTunnel {
    /// What the product reads.
    pub(super) fn view(&self) -> TunnelView {
        TunnelView {
            status: self.status.subscribe(),
        }
    }

    /// The setting this run serves, when complete.
    pub(super) fn setting(&self) -> Option<TunnelSetting> {
        self.setting.clone()
    }

    /// What this run serves, for Settings; see `running`.
    pub(super) fn running(&self) -> watch::Receiver<Option<TunnelSetting>> {
        self.running.subscribe()
    }

    pub(super) fn ingress(&self) -> SocketAddr {
        self.ingress
    }

    /// The ingress cannot be bound: nothing runs, and a program an earlier
    /// run left running is ended, since it would go on forwarding to
    /// whatever holds the port.
    pub(super) fn ingress_failed(self) -> JoinHandle<()> {
        self.running.send_replace(None);
        self.status
            .send_replace(TunnelStatus::Failed(TunnelFailure::Unavailable("ingress")));
        tokio::spawn(release(self.files))
    }
}

fn service_name() -> KeptName {
    KeptName::new(SERVICE_NAME).expect("the service's name is a kept name")
}

/// shepherd's records of the program kept across runs.
fn kept(files: &TunnelFiles) -> Result<KeptProcesses, KeptError> {
    shepherd::kept_processes(files.kept_directory(), OWNER)
}

/// End the program a last run left running, if it is still that program.
async fn release(files: TunnelFiles) {
    forget_unkept(&files);
    // Nothing was ever kept: nothing to look up, and nothing is created.
    if !files.kept_directory().is_dir() {
        return;
    }
    let kept = match kept(&files) {
        Ok(kept) => kept,
        Err(KeptError::Unsupported) => return,
        Err(error) => {
            tracing::warn!(%error, "the tunnel's kept program could not be looked up");
            return;
        }
    };
    match kept.adopt(&service_name()).await {
        Ok(Some(process)) => {
            let ended = process.terminate(TerminateOptions::default()).await;
            if !ended.outcome.is_verified() {
                tracing::warn!(
                    outcome = ?ended.outcome,
                    "the tunnel's kept program could not be confirmed gone"
                );
            }
        }
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "the tunnel's kept program could not be ended"),
    }
}

/// Before shepherd kept the program the gateway recorded it itself. That
/// record is dropped, not followed: the program it names, if one still
/// runs, is left to the person (alpha data, `docs/design/auth/tunnel.md`).
fn forget_unkept(files: &TunnelFiles) {
    let record = files.directory().join(UNKEPT_RECORD);
    if std::fs::remove_file(&record).is_ok() {
        tracing::warn!(
            "a tunnel program recorded before shepherd kept it is no longer followed; \
             end any cloudflared still running from an earlier version"
        );
    }
}

/// The tunnel's status for a shepherd status, or `None` for one that
/// changes nothing a reader sees: stopping, as the gateway stops, and a
/// start after a failure, which keeps reading `failed` with its reason
/// until the tunnel is ready.
fn tunnel_status(
    status: &ServiceStatus<nessa_protocol::tunnel::TunnelAddress>,
    refused: Option<TunnelFailure>,
) -> Option<TunnelStatus> {
    match status {
        ServiceStatus::Starting { attempt } => (*attempt <= 1).then_some(TunnelStatus::Starting),
        ServiceStatus::Ready { value, .. } => Some(TunnelStatus::Ready(value.clone())),
        // The reason's words are logged by shepherd; why it failed is read
        // from which step failed, and a refused launch from its own reason.
        ServiceStatus::Backoff { kind, .. } | ServiceStatus::Failed { kind, .. } => {
            Some(TunnelStatus::Failed(match kind {
                ServiceFailureKind::Launch => refused.unwrap_or(TunnelFailure::Start),
                ServiceFailureKind::ReadyCheck | ServiceFailureKind::NotReadyInTime => {
                    TunnelFailure::NotReady
                }
                ServiceFailureKind::Start
                | ServiceFailureKind::ExitedBeforeReady
                | ServiceFailureKind::Exited => TunnelFailure::Start,
            }))
        }
        ServiceStatus::Stopping | ServiceStatus::Stopped | ServiceStatus::Released { .. } => None,
    }
}

/// Read the tunnel `config` turns on, with its token.
pub(super) fn prepare(namespace: &Path, config: &TunnelConfig) -> PreparedTunnel {
    let files = TunnelFiles::new(namespace);
    let (status, _) = watch::channel(TunnelStatus::Starting);
    let tunnel = match (config.mode, &config.hostname) {
        (TunnelMode::Quick, _) => Some(CloudflareTunnel::Quick),
        (TunnelMode::Named, Some(address)) => match files.read_token() {
            Ok(Some(token)) => Some(CloudflareTunnel::Named {
                address: address.clone(),
                token,
            }),
            Ok(None) | Err(_) => None,
        },
        // The configuration's check refuses a named tunnel with no hostname.
        (TunnelMode::Named, None) => None,
    };
    if tunnel.is_none() {
        tracing::warn!("the named tunnel's token could not be read; the tunnel is not started");
        status.send_replace(TunnelStatus::Failed(TunnelFailure::Unavailable("token")));
    }
    let setting = tunnel.map(|tunnel| TunnelSetting {
        ingress: config.ingress,
        provider: TunnelProvider::Cloudflare(tunnel),
    });
    let (running, _) = watch::channel(setting.clone());
    PreparedTunnel {
        files,
        ingress: config.ingress,
        setting,
        status,
        running,
    }
}

/// The provider's program, kept up until `stop`, when it then is left
/// running for the next run.
pub(super) fn start(
    prepared: PreparedTunnel,
    mut stop: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let PreparedTunnel {
        files,
        ingress,
        setting,
        status,
        running,
    } = prepared;
    let setting = setting?;
    forget_unkept(&files);
    let kept = match kept(&files) {
        Ok(kept) => kept,
        Err(error) => {
            tracing::warn!(%error, "the tunnel's program cannot be kept; the tunnel is not started");
            let reason = match error {
                KeptError::Unsupported => "unsupported",
                _ => "storage",
            };
            status.send_replace(TunnelStatus::Failed(TunnelFailure::Unavailable(reason)));
            // Nothing runs, so the same setting saved again is tried again.
            running.send_replace(None);
            return None;
        }
    };
    let archives: MakeArchives = Arc::new(|| {
        HttpsArchives::new()
            .ok()
            .map(|archives| Box::new(archives) as Box<dyn ArchiveSource>)
    });
    let TunnelProvider::Cloudflare(tunnel) = setting.provider;
    let refused = LaunchRefusal::default();
    let service = CloudflareService::new(
        files,
        Program::chosen(),
        archives,
        tunnel,
        ingress,
        Arc::new(ShepherdKept::new(kept.clone(), service_name())),
        refused.clone(),
    );
    Some(tokio::spawn(async move {
        let supervisor = shepherd::service_supervisor(&SupervisorBuilder::new().build());
        let options = ServiceOptions {
            ready_within: READY_WITHIN,
            kept: Some(kept),
            ..ServiceOptions::default()
        };
        let handle = supervisor
            .start(service_name(), service, options)
            .expect("one tunnel service per run");
        let mut watching = handle.status();
        loop {
            // The status first: a refusal is recorded before shepherd
            // publishes its backoff, and not again until that wait is over.
            let latest = watching.borrow_and_update().clone();
            let refused = *refused.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(now) = tunnel_status(&latest, refused) {
                status.send_replace(now);
            }
            tokio::select! {
                changed = watching.changed() => if changed.is_err() {
                    return;
                },
                // A dropped stop is a stop too.
                _ = stop.wait_for(|stopped| *stopped) => break,
            }
        }
        handle.stop(ServiceStop::Leave).await;
    }))
}

/// With the tunnel off, end a program an earlier run left running.
pub(super) fn release_kept(namespace: &Path) -> JoinHandle<()> {
    tokio::spawn(release(TunnelFiles::new(namespace)))
}

#[cfg(test)]
#[path = "../../tests/composition/tunnel.rs"]
mod tests;
