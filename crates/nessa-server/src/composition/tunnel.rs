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
use crate::agent_install::application::ArchiveSource;
use crate::agent_install::infrastructure::HttpsArchives;
use crate::services::application::{Backoff, ServiceFailure, ServiceStatus, ServiceSupervisor};
use crate::services::infrastructure::KeptHost;
use crate::timer::{Timer, TokioTimer};
use crate::tunnel::application::{
    CloudflareTunnel, TunnelMode, TunnelProvider, TunnelSetting, TunnelStatus, TunnelView,
};
use crate::tunnel::infrastructure::cloudflare::{CloudflareService, MakeArchives, Program};
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
            .send_replace(ServiceStatus::Failed(ServiceFailure::Unavailable(
                "ingress",
            )));
        let host = kept_host(&self.files);
        tokio::spawn(async move { host.release().await })
    }
}

fn kept_host(files: &TunnelFiles) -> Arc<KeptHost> {
    KeptHost::new(
        files.directory().to_owned(),
        PROGRAM_NAME,
        shepherd::process_observer(),
        Arc::new(TokioTimer::new()),
    )
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

/// The provider's program, kept up until `stop`.
pub(super) fn start(
    prepared: PreparedTunnel,
    stop: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let PreparedTunnel {
        files,
        ingress,
        setting,
        status,
        running: _,
    } = prepared;
    let setting = setting?;
    let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
    let archives: MakeArchives = Arc::new(|| {
        HttpsArchives::new()
            .ok()
            .map(|archives| Box::new(archives) as Box<dyn ArchiveSource>)
    });
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
    let host = kept_host(&TunnelFiles::new(namespace));
    tokio::spawn(async move { host.release().await })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::application::{Halt, HostedEnd, Launch, ProcessHost};
    use std::time::Duration;
    use tokio::sync::oneshot;

    fn alive(pid: u32) -> bool {
        // Signal 0 checks for the process without signalling it.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    /// A bind failure ends the program an earlier run left running, which
    /// would otherwise go on forwarding to whatever holds the port, and
    /// leaves Settings comparing with no running tunnel.
    #[tokio::test]
    async fn an_ingress_that_cannot_be_bound_ends_the_kept_program() {
        let namespace = tempfile::tempdir().unwrap();
        let files = TunnelFiles::new(namespace.path());
        std::fs::create_dir_all(files.directory()).unwrap();
        let host = kept_host(&files);
        let (started, running) = oneshot::channel();
        let (leave, halted) = oneshot::channel();
        let run = tokio::spawn(host.run(
            "quick".into(),
            Launch {
                program: "/bin/sleep".into(),
                args: vec!["60".into()],
                environment: vec![],
                directory: files.directory().to_owned(),
            },
            started,
            Box::pin(async move { halted.await.unwrap_or(Halt::End) }),
        ));
        running.await.unwrap();
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(files.directory().join("process.json")).unwrap())
                .unwrap();
        let pid = record["pid"].as_u64().unwrap() as u32;
        leave.send(Halt::Leave).unwrap();
        assert_eq!(run.await.unwrap(), HostedEnd::Left);
        assert!(alive(pid), "left running, as a gateway stop leaves it");

        let prepared = prepare(
            namespace.path(),
            &TunnelConfig {
                ingress: "127.0.0.1:47651".parse().unwrap(),
                mode: crate::tunnel::application::TunnelMode::Quick,
                hostname: None,
            },
        );
        let status = prepared.view().status;
        let settings = prepared.running();
        assert!(settings.borrow().is_some());
        prepared.ingress_failed().await.unwrap();

        tokio::time::timeout(Duration::from_secs(15), async {
            while alive(pid) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the kept program is ended");
        assert!(!files.directory().join("process.json").exists());
        assert!(
            settings.borrow().is_none(),
            "nothing runs for Settings to hold"
        );
        assert_eq!(
            *status.borrow(),
            ServiceStatus::Failed(ServiceFailure::Unavailable("ingress"))
        );
    }
}
