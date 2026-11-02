//! Setting the hub's machines up from Devices: suggesting the ones the
//! person's SSH configuration names, connecting one (`devices.connectSsh`),
//! and choosing the default device new conversations run on
//! (`devices.setDefault`).
//!
//! ```text
//! devices.sshHosts ──▶ DeviceSetup::ssh_hosts
//!   ──▶ ssh_config::suggestions (SshConfigSource, less this run's hosts)
//!   ──▶ the connections under way, from memory
//! devices.connectSsh(host) ──▶ DeviceSetup::connect ──▶ Installing (answered)
//!   └─ task ──▶ HostInstall::install(host, operation)   audited as a lease's install
//!        failed ──▶ Failed(refusal), nothing saved
//!        ──▶ DeviceSettings::add_ssh_host                 config.json sshHosts, audited
//!            this run serves it ──▶ Connected
//!            otherwise ──▶ Restarting: the gateway starts again in place with it
//! devices.setDefault(environment) ──▶ DeviceSetup::set_default
//!   ──▶ named here? (null, a configured SSH host, or a peer: name)
//!   ──▶ DeviceSettings::set_default                       config.json defaultEnvironment, audited
//!   ──▶ the default watch ──▶ conversation creation, the agent catalog
//! ```
//!
//! Which SSH hosts conversations may run on is read once, at startup, as
//! every environment is composed then: a new host applies by starting
//! again, as native linking does. The default device is read at each
//! creation, so it applies at once. A connection's progress lives only in
//! this run; the one that restarts for it shows the host as configured.
use super::ssh_config::{resolved, suggestions, SshConfigSource, SshHost};
use crate::device_pairing::application::{LinkingApply, LinkingCaller, LinkingError};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, PeerEnvironment, SshDestination};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::watch;

/// The most connections kept at once: as many as config.json may name hosts.
pub(crate) const MAX_CONNECTS: usize = 16;

pub(crate) type SetupFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Installs this build on a host, as a conversation's first lease there
/// would. Implemented in composition over the SSH host installer.
pub(crate) trait HostInstall: Send + Sync {
    /// Make sure this build is on `host`; `operation` names the install in
    /// the environment audit.
    fn install<'a>(
        &'a self,
        host: &'a SshDestination,
        operation: &'a str,
    ) -> SetupFuture<'a, Result<(), LeaseRefusal>>;
}

/// Where the Devices settings are saved: config.json, under its one lock,
/// each change audited. Implemented in composition.
pub(crate) trait DeviceSettings: Send + Sync {
    /// Add `host` to `sshHosts`; restarting when this run does not serve it.
    fn add_ssh_host(
        &self,
        caller: LinkingCaller,
        operation: String,
        host: SshDestination,
    ) -> SetupFuture<'_, Result<LinkingApply, LinkingError>>;
    /// Save `environment` as `defaultEnvironment`, `None` removing it.
    fn set_default(
        &self,
        caller: LinkingCaller,
        environment: Option<String>,
    ) -> SetupFuture<'_, Result<(), LinkingError>>;
}

/// Where one connection stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectState {
    Installing,
    /// Installed and saved; the gateway is starting again to serve it.
    Restarting,
    /// Installed, and this run already serves it.
    Connected,
    Failed(ConnectRefusal),
}

/// Why a connection failed. Nothing was saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectRefusal {
    /// The install, as a lease's would be refused.
    Install(LeaseRefusal),
    /// config.json already names as many hosts as it may.
    HostLimit,
    /// config.json is held by another change, or the gateway restarts.
    Busy,
    /// config.json or its audit could not be written.
    Unavailable,
}

/// One connection, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Connect {
    pub(crate) host: SshDestination,
    pub(crate) state: ConnectState,
}

/// Why a default was not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefaultRefusal {
    /// Not this machine, a configured SSH host or a paired environment's
    /// name.
    NotADevice,
    Busy,
    Unavailable,
}

/// The Devices page's setup of the hub's machines.
pub(crate) struct DeviceSetup {
    source: Arc<dyn SshConfigSource>,
    /// The SSH hosts this run serves, as config.json named them at startup.
    hosts: Vec<SshDestination>,
    install: Arc<dyn HostInstall>,
    settings: Arc<dyn DeviceSettings>,
    connects: Mutex<BTreeMap<SshDestination, ConnectState>>,
    default: watch::Sender<Option<String>>,
}

impl DeviceSetup {
    /// `default` holds the saved default as startup read it; its receivers
    /// are what creation and the catalog read.
    pub(crate) fn new(
        source: Arc<dyn SshConfigSource>,
        hosts: Vec<SshDestination>,
        install: Arc<dyn HostInstall>,
        settings: Arc<dyn DeviceSettings>,
        default: watch::Sender<Option<String>>,
    ) -> Self {
        Self {
            source,
            hosts,
            install,
            settings,
            connects: Mutex::new(BTreeMap::new()),
            default,
        }
    }

    /// The SSH hosts this run serves, each as the configuration resolves
    /// it. Reads files: call it off the async workers.
    pub(crate) fn configured(&self) -> Vec<SshHost> {
        self.hosts
            .iter()
            .map(|host| resolved(self.source.as_ref(), host))
            .collect()
    }

    /// The machines to suggest, and the connections under way. Reads
    /// files: call it off the async workers.
    pub(crate) fn ssh_hosts(&self) -> (Vec<SshHost>, Vec<Connect>) {
        let suggested = suggestions(self.source.as_ref(), &self.hosts);
        (suggested, self.connects())
    }

    fn connects(&self) -> Vec<Connect> {
        self.connects
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(host, state)| Connect {
                host: host.clone(),
                state: *state,
            })
            .collect()
    }

    /// The saved default device; `None` for this machine.
    pub(crate) fn default_environment(&self) -> Option<String> {
        self.default.borrow().clone()
    }

    /// Start connecting `host` for `caller`, unless it is already under way:
    /// answered with where it stands now.
    pub(crate) fn connect(
        self: &Arc<Self>,
        caller: LinkingCaller,
        host: SshDestination,
    ) -> ConnectState {
        {
            let mut connects = self.connects.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(state @ (ConnectState::Installing | ConnectState::Restarting)) =
                connects.get(&host)
            {
                return *state;
            }
            if !connects.contains_key(&host) && connects.len() >= MAX_CONNECTS {
                // Make room by forgetting an ended one; with none ended, as
                // many are under way as config.json could take.
                let ended = connects
                    .iter()
                    .find(|(_, state)| {
                        matches!(state, ConnectState::Failed(_) | ConnectState::Connected)
                    })
                    .map(|(host, _)| host.clone());
                match ended {
                    Some(ended) => {
                        connects.remove(&ended);
                    }
                    None => return ConnectState::Failed(ConnectRefusal::HostLimit),
                }
            }
            connects.insert(host.clone(), ConnectState::Installing);
        }
        let setup = self.clone();
        tokio::spawn(async move {
            let state = setup.connected(caller, &host).await;
            setup
                .connects
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(host, state);
        });
        ConnectState::Installing
    }

    async fn connected(&self, caller: LinkingCaller, host: &SshDestination) -> ConnectState {
        let operation = uuid::Uuid::new_v4().to_string();
        if let Err(refusal) = self.install.install(host, &operation).await {
            tracing::info!(host = host.as_str(), ?refusal, "the host could not be set up");
            return ConnectState::Failed(ConnectRefusal::Install(refusal));
        }
        match self
            .settings
            .add_ssh_host(caller, operation, host.clone())
            .await
        {
            Ok(LinkingApply::Restarting) => ConnectState::Restarting,
            Ok(LinkingApply::Unchanged) => ConnectState::Connected,
            Err(LinkingError::HostLimit) => ConnectState::Failed(ConnectRefusal::HostLimit),
            Err(LinkingError::Busy) => ConnectState::Failed(ConnectRefusal::Busy),
            Err(_) => ConnectState::Failed(ConnectRefusal::Unavailable),
        }
    }

    /// Save `environment` as the default device for `caller`: `None` for
    /// this machine, a configured SSH host, or a paired environment's
    /// `peer:` name. A paired environment's key is not checked against the
    /// pairings: one no longer paired is never connected, and a
    /// conversation then stays here.
    pub(crate) async fn set_default(
        &self,
        caller: LinkingCaller,
        environment: Option<String>,
    ) -> Result<Option<String>, DefaultRefusal> {
        if let Some(name) = environment.as_deref() {
            let device = PeerEnvironment::parse(name).is_ok()
                || SshDestination::new(name).is_ok_and(|host| self.hosts.contains(&host));
            if !device {
                return Err(DefaultRefusal::NotADevice);
            }
        }
        match self.settings.set_default(caller, environment.clone()).await {
            Ok(()) => {
                self.default.send_replace(environment.clone());
                Ok(environment)
            }
            Err(LinkingError::Busy) => Err(DefaultRefusal::Busy),
            Err(_) => Err(DefaultRefusal::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::application::ssh_config::SshConfigSource;
    use std::time::Duration;

    struct Config(&'static str);
    impl SshConfigSource for Config {
        fn main(&self) -> Option<String> {
            Some(self.0.to_owned())
        }
        fn include(&self, _pattern: &str, _limit: usize) -> Vec<String> {
            Vec::new()
        }
    }

    #[derive(Default)]
    struct Install {
        refuse: Option<LeaseRefusal>,
        asked: Mutex<Vec<(String, String)>>,
        hold: Option<Arc<tokio::sync::Notify>>,
    }
    impl HostInstall for Install {
        fn install<'a>(
            &'a self,
            host: &'a SshDestination,
            operation: &'a str,
        ) -> SetupFuture<'a, Result<(), LeaseRefusal>> {
            Box::pin(async move {
                self.asked
                    .lock()
                    .unwrap()
                    .push((host.as_str().to_owned(), operation.to_owned()));
                if let Some(hold) = &self.hold {
                    hold.notified().await;
                }
                self.refuse.map_or(Ok(()), Err)
            })
        }
    }

    #[derive(Default)]
    struct Settings {
        added: Mutex<Vec<(String, String)>>,
        defaults: Mutex<Vec<Option<String>>>,
        refuse: Option<LinkingError>,
    }
    impl DeviceSettings for Settings {
        fn add_ssh_host(
            &self,
            _caller: LinkingCaller,
            operation: String,
            host: SshDestination,
        ) -> SetupFuture<'_, Result<LinkingApply, LinkingError>> {
            Box::pin(async move {
                if let Some(error) = self.refuse {
                    return Err(error);
                }
                self.added
                    .lock()
                    .unwrap()
                    .push((host.as_str().to_owned(), operation));
                Ok(if host.as_str() == "served" {
                    LinkingApply::Unchanged
                } else {
                    LinkingApply::Restarting
                })
            })
        }
        fn set_default(
            &self,
            _caller: LinkingCaller,
            environment: Option<String>,
        ) -> SetupFuture<'_, Result<(), LinkingError>> {
            Box::pin(async move {
                if let Some(error) = self.refuse {
                    return Err(error);
                }
                self.defaults.lock().unwrap().push(environment);
                Ok(())
            })
        }
    }

    fn caller() -> LinkingCaller {
        LinkingCaller {
            organization_id: "org".into(),
            principal_id: "owner".into(),
            credential_id: "credential".into(),
        }
    }

    fn host(name: &str) -> SshDestination {
        SshDestination::new(name).unwrap()
    }

    fn setup(
        install: Install,
        settings: Settings,
    ) -> (Arc<DeviceSetup>, Arc<Install>, Arc<Settings>, watch::Receiver<Option<String>>) {
        let install = Arc::new(install);
        let settings = Arc::new(settings);
        let (default, read) = watch::channel(None);
        let setup = Arc::new(DeviceSetup::new(
            Arc::new(Config("Host box served\nHost gh\n  HostName github.com\n")),
            vec![host("served")],
            install.clone(),
            settings.clone(),
            default,
        ));
        (setup, install, settings, read)
    }

    async fn settled(setup: &DeviceSetup, name: &str) -> ConnectState {
        for _ in 0..200 {
            let state = setup
                .connects()
                .into_iter()
                .find(|each| each.host.as_str() == name)
                .map(|each| each.state);
            if !matches!(state, Some(ConnectState::Installing) | None) {
                return state.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("{name} never settled");
    }

    #[tokio::test]
    async fn suggestions_leave_out_served_hosts_and_git_servers() {
        let (setup, ..) = setup(Install::default(), Settings::default());
        let (suggested, connects) = setup.ssh_hosts();
        assert_eq!(
            suggested.iter().map(|each| each.host.as_str()).collect::<Vec<_>>(),
            ["box"]
        );
        assert!(connects.is_empty());
        assert_eq!(setup.configured()[0].host.as_str(), "served");
    }

    #[tokio::test]
    async fn a_connection_installs_then_saves_with_one_operation() {
        let (setup, install, settings, _) = setup(Install::default(), Settings::default());
        assert_eq!(setup.connect(caller(), host("box")), ConnectState::Installing);
        assert_eq!(settled(&setup, "box").await, ConnectState::Restarting);
        let installed = install.asked.lock().unwrap().clone();
        let added = settings.added.lock().unwrap().clone();
        assert_eq!(installed.len(), 1);
        // The install and the change are one operation in the audit.
        assert_eq!(installed, added);
        // One this run serves is installed and connected, with no restart.
        setup.connect(caller(), host("served"));
        assert_eq!(settled(&setup, "served").await, ConnectState::Connected);
    }

    #[tokio::test]
    async fn a_refused_install_saves_nothing_and_may_be_tried_again() {
        let (setup, _, settings, _) = setup(
            Install {
                refuse: Some(LeaseRefusal::EnvironmentUnreachable),
                ..Install::default()
            },
            Settings::default(),
        );
        setup.connect(caller(), host("box"));
        assert_eq!(
            settled(&setup, "box").await,
            ConnectState::Failed(ConnectRefusal::Install(
                LeaseRefusal::EnvironmentUnreachable
            ))
        );
        assert!(settings.added.lock().unwrap().is_empty());
        assert_eq!(setup.connect(caller(), host("box")), ConnectState::Installing);
    }

    #[tokio::test]
    async fn a_connection_under_way_is_not_started_twice() {
        let hold = Arc::new(tokio::sync::Notify::new());
        let (setup, install, ..) = setup(
            Install {
                hold: Some(hold.clone()),
                ..Install::default()
            },
            Settings::default(),
        );
        setup.connect(caller(), host("box"));
        assert_eq!(setup.connect(caller(), host("box")), ConnectState::Installing);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(install.asked.lock().unwrap().len(), 1);
        hold.notify_one();
        assert_eq!(settled(&setup, "box").await, ConnectState::Restarting);
    }

    #[tokio::test]
    async fn a_full_or_busy_config_fails_the_connection() {
        for (error, refusal) in [
            (LinkingError::HostLimit, ConnectRefusal::HostLimit),
            (LinkingError::Busy, ConnectRefusal::Busy),
            (LinkingError::AuditUnavailable, ConnectRefusal::Unavailable),
        ] {
            let (setup, ..) = setup(
                Install::default(),
                Settings {
                    refuse: Some(error),
                    ..Settings::default()
                },
            );
            setup.connect(caller(), host("box"));
            assert_eq!(settled(&setup, "box").await, ConnectState::Failed(refusal));
        }
    }

    #[tokio::test]
    async fn the_default_is_a_device_and_moves_the_watch_once_saved() {
        let (setup, _, settings, read) = setup(Install::default(), Settings::default());
        let peer = format!("peer:{}", "ab".repeat(32));
        assert_eq!(
            setup.set_default(caller(), Some("box".into())).await,
            Err(DefaultRefusal::NotADevice),
            "a host this run does not serve"
        );
        assert_eq!(
            setup.set_default(caller(), Some("peer:any".into())).await,
            Err(DefaultRefusal::NotADevice)
        );
        assert_eq!(
            setup.set_default(caller(), Some("served".into())).await,
            Ok(Some("served".into()))
        );
        assert_eq!(read.borrow().as_deref(), Some("served"));
        setup.set_default(caller(), Some(peer.clone())).await.unwrap();
        setup.set_default(caller(), None).await.unwrap();
        assert_eq!(*read.borrow(), None);
        assert_eq!(
            *settings.defaults.lock().unwrap(),
            [Some("served".into()), Some(peer), None]
        );
        assert_eq!(setup.default_environment(), None);
    }

    #[tokio::test]
    async fn a_default_not_saved_leaves_the_watch() {
        let (setup, _, _, read) = setup(
            Install::default(),
            Settings {
                refuse: Some(LinkingError::Busy),
                ..Settings::default()
            },
        );
        assert_eq!(
            setup.set_default(caller(), Some("served".into())).await,
            Err(DefaultRefusal::Busy)
        );
        assert_eq!(*read.borrow(), None);
    }
}
