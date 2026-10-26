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
//!          ServiceSupervisor(CloudflareService, KeptHost) ──▶ status ──▶ pairing.tunnel, server.reach
//! stop:    both asked to stop; the provider's program is left running for
//!          the next run to take back (KeptHost)
//! off:     a program a last run left running is ended (release)
//! ```
//!
//! Arrows are construction and the order things start. The ingress forwards
//! into the native listener, whose tunnel edge admits only paired keys
//! (`device_pairing::infrastructure::connection`).
use super::runtime_config::TunnelConfig;
use crate::agent_install::infrastructure::HttpsArchives;
use crate::services::application::{Backoff, ServiceFailure, ServiceStatus, ServiceSupervisor};
use crate::services::infrastructure::KeptHost;
use crate::timer::{Timer, TokioTimer};
use crate::tunnel::application::{
    CloudflareTunnel, TunnelMode, TunnelProvider, TunnelSetting, TunnelStatus, TunnelView,
};
use crate::tunnel::infrastructure::cloudflare::{CloudflareService, Program};
use crate::tunnel::infrastructure::files::TunnelFiles;
use std::{net::SocketAddr, path::Path, sync::Arc};
use tokio::{sync::watch, task::JoinHandle};

/// The provider's program, by name: its log is `<tunnel>/cloudflared.log`.
const PROGRAM_NAME: &str = "cloudflared";

/// The tunnel as this run serves it, before anything is bound.
pub(super) struct PreparedTunnel {
    files: TunnelFiles,
    ingress: SocketAddr,
    /// `None` when the setting could not be completed (a named tunnel's
    /// token unreadable): the status says why, and nothing runs.
    setting: Option<TunnelSetting>,
    status: watch::Sender<TunnelStatus>,
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

    pub(super) fn ingress(&self) -> SocketAddr {
        self.ingress
    }
}

/// Read the tunnel `config` turns on, with its token.
pub(super) fn prepare(namespace: &Path, config: &TunnelConfig) -> PreparedTunnel {
    let files = TunnelFiles::new(namespace);
    let (status, _) = watch::channel(ServiceStatus::Starting);
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
        status.send_replace(ServiceStatus::Failed(ServiceFailure::Unavailable("token")));
    }
    PreparedTunnel {
        files,
        ingress: config.ingress,
        setting: tunnel.map(|tunnel| TunnelSetting {
            ingress: config.ingress,
            provider: TunnelProvider::Cloudflare(tunnel),
        }),
        status,
    }
}

/// The provider's program, kept up until `stop`.
pub(super) fn start(prepared: PreparedTunnel, stop: watch::Receiver<bool>) -> Option<JoinHandle<()>> {
    let PreparedTunnel {
        files,
        ingress,
        setting,
        status,
    } = prepared;
    let setting = setting?;
    let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
    let archives = match HttpsArchives::new() {
        Ok(archives) => Arc::new(archives),
        Err(_) => {
            status.send_replace(ServiceStatus::Failed(ServiceFailure::Unavailable("download")));
            return None;
        }
    };
    let TunnelProvider::Cloudflare(tunnel) = setting.provider;
    let service = Arc::new(CloudflareService::new(
        files.clone(),
        Program::chosen(),
        archives,
        tunnel,
        ingress,
    ));
    let host = KeptHost::new(
        files.directory().to_owned(),
        PROGRAM_NAME,
        shepherd::process_observer(),
        timer.clone(),
    );
    let supervisor = ServiceSupervisor {
        service,
        host: Arc::new(host),
        timer,
        status,
        backoff: Backoff::default(),
    };
    Some(tokio::spawn(supervisor.run(stop)))
}

/// With the tunnel off, end a program an earlier run left running.
pub(super) fn release_kept(namespace: &Path) -> JoinHandle<()> {
    let files = TunnelFiles::new(namespace);
    let host = KeptHost::new(
        files.directory().to_owned(),
        PROGRAM_NAME,
        shepherd::process_observer(),
        Arc::new(TokioTimer::new()),
    );
    tokio::spawn(async move { host.release().await })
}
