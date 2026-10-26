//! `pairing.setLinking`: native linking turned on or off from Settings, saved
//! in `config.json` and applied by starting the gateway again.
//!
//! ```text
//! NativeLinking::set ──record requested──▶ LinkingAudit
//!                    ──lock / read───────▶ ConfigJson (config.json and its lock, shared with the MCP server settings)
//!                    ──bind once─────────▶ the listen address (turning on, when this run does not listen)
//!                    ──publish───────────▶ ConfigJson, only `native` changed
//!                    ──record outcome────▶ LinkingAudit, still holding the lock
//!                    ──schedule──────────▶ SelfRestart, when the running gateway differs
//!                    ◀─restart ended─────  SelfRestart, recorded as the change's restart
//! ```
//!
//! Turning it on keeps a listen address already in the file, or names
//! [`DEFAULT_NATIVE_LISTEN`]; the loopback address Settings itself used to
//! name ([`SUPERSEDED_NATIVE_LISTEN`]) is replaced by it, since no phone
//! could reach it. Turning it off writes `"enabled": false` and
//! keeps the addresses, so on again finds them. An address this machine cannot
//! bind is refused before anything is saved: a saved one would leave a gateway
//! that cannot start (design row S10 stops startup on a native bind failure).
//! A change is refused busy while a restart is held or the gateway is
//! stopping, so the gateway that comes back is the one the last answer named.
//!
//! `pairing.setTunnel` changes `native.tunnel` the same way, under the same
//! lock and audit (its own subject): a named tunnel's token is written to
//! its private file first (`tunnel::infrastructure::files`), then the
//! section, so a saved named tunnel never lacks its token; turning it on
//! needs linking on, and an ingress this run does not hold is bind-checked.
//! It restarts when the saved tunnel's identity differs from the one this
//! run serves.
use super::runtime_config::{TunnelSection, DEFAULT_NATIVE_LISTEN, SUPERSEDED_NATIVE_LISTEN};
use crate::core::self_restart::{RestartEnd, RestartReport, SelfRestart};
use crate::device_pairing::application::{
    LinkingApply, LinkingAudit, LinkingAuditPhase, LinkingAuditRecord, LinkingCaller,
    LinkingChange, LinkingError, LinkingRestart, LinkingSubject, ListenMoved, NativeLinking,
    SavedTunnel, TunnelChange, TunnelRequest,
};
use crate::mcp_servers::application::StoreError;
use crate::mcp_servers::infrastructure::{ConfigJson, Published};
use crate::tunnel::application::{
    CloudflareTunnel, TunnelMode, TunnelProvider, TunnelSetting, TunnelToken,
    DEFAULT_TUNNEL_INGRESS,
};
use crate::tunnel::infrastructure::files::TunnelFiles;
use serde_json::{Map, Value};
use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

/// How long the answer has to reach the caller before the restart closes its
/// socket. The caller reads the state again once it reconnects, so a reply
/// lost to a slow socket costs a read, not the change.
const ANSWER_GRACE: Duration = Duration::from_millis(250);

/// `config.json`'s `native` section, its audit, and this run's restart.
pub(super) struct ConfigLinking {
    config: Arc<ConfigJson>,
    audit: Arc<dyn LinkingAudit>,
    /// Whether this run serves native pairing: what a change is compared
    /// with to decide whether to start again.
    running: bool,
    restart: Arc<SelfRestart>,
    /// The tunnel's files, where its token is kept; `None` answers
    /// tunnel changes unavailable.
    tunnel_files: Option<TunnelFiles>,
    /// The tunnel this run serves, if any: what a tunnel change is compared
    /// with.
    running_tunnel: Option<TunnelSetting>,
}

/// One change.
#[derive(Clone)]
enum Change {
    Linking(bool),
    Tunnel(TunnelRequest),
}

impl Change {
    fn subject(&self) -> LinkingSubject {
        match self {
            Self::Linking(_) => LinkingSubject::Linking,
            Self::Tunnel(request) => LinkingSubject::Tunnel { mode: request.mode },
        }
    }
    fn enabled(&self) -> bool {
        match self {
            Self::Linking(enabled) => *enabled,
            Self::Tunnel(request) => request.enabled,
        }
    }
}

impl ConfigLinking {
    pub(super) fn new(
        config: Arc<ConfigJson>,
        audit: Arc<dyn LinkingAudit>,
        running: bool,
        restart: Arc<SelfRestart>,
    ) -> Self {
        Self {
            config,
            audit,
            running,
            restart,
            tunnel_files: None,
            running_tunnel: None,
        }
    }

    /// Change the tunnel too: its token kept in `files`, compared with
    /// `running`, the tunnel this run serves.
    pub(super) fn with_tunnel(mut self, files: TunnelFiles, running: Option<TunnelSetting>) -> Self {
        self.tunnel_files = Some(files);
        self.running_tunnel = running;
        self
    }

    /// Whether this run serves what `change` is to.
    fn serves(&self, change: &Change) -> bool {
        match change {
            Change::Linking(_) => self.running,
            Change::Tunnel(_) => self.running_tunnel.is_some(),
        }
    }

    /// Keep one record, off the async workers: each is a synced file.
    async fn record(
        &self,
        operation_id: &str,
        caller: &LinkingCaller,
        change: &Change,
        phase: LinkingAuditPhase,
    ) -> Result<(), LinkingError> {
        let audit = self.audit.clone();
        let record = LinkingAuditRecord {
            operation_id: operation_id.to_owned(),
            caller: caller.clone(),
            subject: change.subject(),
            enabled: change.enabled(),
            running: self.serves(change),
            phase,
        };
        tokio::task::spawn_blocking(move || audit.record(&record))
            .await
            .map_err(|_| LinkingError::AuditUnavailable)?
            .map_err(|_| LinkingError::AuditUnavailable)
    }

    /// Save `change` under the lock, held until the outcome is recorded and
    /// any restart scheduled, so no other change lands in between.
    async fn change(
        &self,
        operation_id: &str,
        caller: &LinkingCaller,
        change: &Change,
    ) -> Result<Saved, LinkingError> {
        let lock = self.config.lock().await.map_err(refusal);
        let saved = match (&lock, change) {
            (Ok(_), Change::Linking(enabled)) => self.save(*enabled).await,
            (Ok(_), Change::Tunnel(request)) => self.save_tunnel(request).await,
            (Err(error), _) => Err(*error),
        };
        let saved = match saved {
            Ok(saved) => saved,
            Err(error) => {
                self.record(operation_id, caller, change, LinkingAuditPhase::Refused(error))
                    .await?;
                return Err(error);
            }
        };
        let Saved {
            applies,
            written,
            moved,
            ..
        } = saved;
        // Recorded before it is answered or applied: a change whose outcome
        // was not kept is saved, and waits for the next start.
        self.record(
            operation_id,
            caller,
            change,
            LinkingAuditPhase::Applied {
                applies,
                written,
                moved,
            },
        )
        .await?;
        if applies == LinkingApply::Restarting
            && self
                .restart
                .schedule(self.restart_report(operation_id, caller, change))
        {
            let restart = self.restart.clone();
            tokio::spawn(async move {
                tokio::time::sleep(ANSWER_GRACE).await;
                restart.request();
            });
        }
        Ok(saved)
    }

    /// How the restart this change asked for ended, recorded with the
    /// change's id. The run is ending, so a record that cannot be kept is
    /// logged and the ending goes on.
    fn restart_report(
        &self,
        operation_id: &str,
        caller: &LinkingCaller,
        change: &Change,
    ) -> RestartReport {
        let audit = self.audit.clone();
        let record = LinkingAuditRecord {
            operation_id: operation_id.to_owned(),
            caller: caller.clone(),
            subject: change.subject(),
            enabled: change.enabled(),
            running: self.serves(change),
            phase: LinkingAuditPhase::Requested,
        };
        Box::new(move |end| {
            let phase = LinkingAuditPhase::Restart(match end {
                RestartEnd::Exec => LinkingRestart::Exec,
                RestartEnd::ExecFailed => LinkingRestart::ExecFailed,
                RestartEnd::Cancelled => LinkingRestart::Cancelled,
            });
            if audit
                .record(&LinkingAuditRecord {
                    phase,
                    ..record.clone()
                })
                .is_err()
            {
                tracing::error!(?end, "the native linking restart could not be recorded");
            }
        })
    }

    /// Write `native` as `enabled`, holding the lock.
    async fn save(&self, enabled: bool) -> Result<Saved, LinkingError> {
        if !self.restart.serving() {
            return Err(LinkingError::Busy);
        }
        let config = self.config.clone();
        let Edited {
            document,
            listen,
            moved,
        } = tokio::task::spawn_blocking(move || {
            config
                .document()
                .map_err(refusal)
                .map(|document| edit(document, enabled))
        })
        .await
        .map_err(|_| LinkingError::Unavailable)??;
        // A running listener holds its own address: only a gateway not
        // serving native pairing has one to try, whether or not the file
        // changes (it may say on already, from a change whose outcome was not
        // kept or from a hand edit). The lock stays held across the bind, so
        // no other change lands while it is tried.
        if let (Some(address), false) = (listen, self.running) {
            bindable(address).await?;
        }
        // A moved address applies only by starting again, even when the run
        // already serves native pairing (at the old address).
        let applies = if enabled == self.running && moved.is_none() {
            LinkingApply::Unchanged
        } else {
            LinkingApply::Restarting
        };
        let Some(document) = document else {
            return Ok(Saved {
                applies,
                written: false,
                moved,
                mode: None,
            });
        };
        let config = self.config.clone();
        let published = tokio::task::spawn_blocking(move || config.publish(document))
            .await
            .map_err(|_| LinkingError::Unavailable)?
            .map_err(refusal)?;
        if published == Published::NotDurable {
            tracing::warn!("native linking saved, but config.json's directory could not be synced");
        }
        Ok(Saved {
            applies,
            written: true,
            moved,
            mode: None,
        })
    }

    /// Write `native.tunnel` as `request` asks, holding the lock: the token
    /// first, then the section.
    async fn save_tunnel(&self, request: &TunnelRequest) -> Result<Saved, LinkingError> {
        if !self.restart.serving() {
            return Err(LinkingError::Busy);
        }
        let files = self.tunnel_files.clone().ok_or(LinkingError::Unavailable)?;
        let config = self.config.clone();
        let asked = request.clone();
        let reading = files.clone();
        let edited = tokio::task::spawn_blocking(move || {
            let document = config.document().map_err(refusal)?;
            let saved = reading
                .read_token()
                .map_err(|_| LinkingError::Unavailable)?;
            edit_tunnel(document, &asked, saved)
        })
        .await
        .map_err(|_| LinkingError::Unavailable)??;
        // The ingress this run binds is held; any other is tried, under the
        // lock, so a saved tunnel never leaves a gateway that cannot start.
        if let Some(setting) = &edited.setting {
            let held = self
                .running_tunnel
                .as_ref()
                .is_some_and(|running| running.ingress == setting.ingress);
            if !held {
                bindable(setting.ingress).await?;
            }
        }
        if let Some(token) = edited.token {
            tokio::task::spawn_blocking(move || files.write_token(&token))
                .await
                .map_err(|_| LinkingError::Unavailable)?
                .map_err(|_| LinkingError::Unavailable)?;
        }
        let applies = if edited.setting.as_ref().map(TunnelSetting::identity)
            == self.running_tunnel.as_ref().map(TunnelSetting::identity)
        {
            LinkingApply::Unchanged
        } else {
            LinkingApply::Restarting
        };
        let written = match edited.document {
            Some(document) => {
                let config = self.config.clone();
                let published = tokio::task::spawn_blocking(move || config.publish(document))
                    .await
                    .map_err(|_| LinkingError::Unavailable)?
                    .map_err(refusal)?;
                if published == Published::NotDurable {
                    tracing::warn!("the tunnel saved, but config.json's directory could not be synced");
                }
                true
            }
            None => false,
        };
        Ok(Saved {
            applies,
            written,
            moved: None,
            mode: Some(edited.mode),
        })
    }

    /// The tunnel as saved, read without the lock: a change in flight is
    /// seen before or after, never half.
    async fn read_tunnel(&self) -> Result<SavedTunnel, LinkingError> {
        let files = self.tunnel_files.clone().ok_or(LinkingError::Unavailable)?;
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            let document = config.document().map_err(refusal)?;
            let section = saved_section(&document)?;
            let has_token = files
                .read_token()
                .map_err(|_| LinkingError::Unavailable)?
                .is_some();
            Ok(match section {
                Some(section) => SavedTunnel {
                    enabled: section.enabled,
                    mode: section.mode.into(),
                    hostname: section.hostname,
                    has_token,
                    ingress: section.ingress.unwrap_or(DEFAULT_TUNNEL_INGRESS),
                },
                None => SavedTunnel {
                    enabled: false,
                    mode: TunnelMode::Quick,
                    hostname: None,
                    has_token,
                    ingress: DEFAULT_TUNNEL_INGRESS,
                },
            })
        })
        .await
        .map_err(|_| LinkingError::Unavailable)?
    }
}

type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, LinkingError>> + Send + 'a>>;

impl NativeLinking for ConfigLinking {
    fn set(&self, caller: LinkingCaller, enabled: bool) -> Answer<'_, LinkingChange> {
        Box::pin(async move {
            let operation_id = uuid::Uuid::new_v4().to_string();
            let change = Change::Linking(enabled);
            self.record(&operation_id, &caller, &change, LinkingAuditPhase::Requested)
                .await?;
            let applies = self.change(&operation_id, &caller, &change).await?.applies;
            tracing::info!(
                enabled,
                ?applies,
                principal = %caller.principal_id,
                "native linking changed from Settings"
            );
            Ok(LinkingChange { enabled, applies })
        })
    }

    fn set_tunnel(&self, caller: LinkingCaller, request: TunnelRequest) -> Answer<'_, TunnelChange> {
        Box::pin(async move {
            let operation_id = uuid::Uuid::new_v4().to_string();
            let enabled = request.enabled;
            let change = Change::Tunnel(request);
            self.record(&operation_id, &caller, &change, LinkingAuditPhase::Requested)
                .await?;
            let saved = self.change(&operation_id, &caller, &change).await?;
            let mode = saved.mode.unwrap_or(TunnelMode::Quick);
            tracing::info!(
                enabled,
                mode = mode.as_str(),
                applies = ?saved.applies,
                principal = %caller.principal_id,
                "the tunnel changed from Settings"
            );
            Ok(TunnelChange {
                enabled,
                mode,
                applies: saved.applies,
            })
        })
    }

    fn tunnel(&self) -> Answer<'_, SavedTunnel> {
        Box::pin(self.read_tunnel())
    }
}

/// What a store refusal means for a change: busy stays busy; anything else
/// is a file that could not be read, checked or written.
fn refusal(error: StoreError) -> LinkingError {
    match error {
        StoreError::Busy => LinkingError::Busy,
        _ => LinkingError::Unavailable,
    }
}

/// What an edit makes of `config.json`.
struct Edited {
    /// The document to publish, when it changes.
    document: Option<Map<String, Value>>,
    /// The address it listens on, when on.
    listen: Option<SocketAddr>,
    /// The listen address it replaced, if any.
    moved: Option<ListenMoved>,
}

/// What a save did: how it applies, whether the file was written, the
/// listen address it replaced, and a tunnel change's saved mode.
struct Saved {
    applies: LinkingApply,
    written: bool,
    moved: Option<ListenMoved>,
    mode: Option<TunnelMode>,
}

/// What a tunnel edit makes of `config.json`.
struct EditedTunnel {
    /// The document to publish, when it changes.
    document: Option<Map<String, Value>>,
    /// The tunnel it turns on, `None` when off.
    setting: Option<TunnelSetting>,
    /// The mode it saves.
    mode: TunnelMode,
    /// A token to write, when one was given and differs from the saved one.
    token: Option<TunnelToken>,
}

/// The saved `native.tunnel`, if any. The file passed the runtime
/// configuration's check, so a section it has parses.
fn saved_section(document: &Map<String, Value>) -> Result<Option<TunnelSection>, LinkingError> {
    document
        .get("native")
        .and_then(|native| native.get("tunnel"))
        .filter(|tunnel| !tunnel.is_null())
        .map(|tunnel| serde_json::from_value(tunnel.clone()).map_err(|_| LinkingError::Unavailable))
        .transpose()
}

/// `document` with `native.tunnel` as `request` asks, given the saved token.
fn edit_tunnel(
    mut document: Map<String, Value>,
    request: &TunnelRequest,
    saved_token: Option<TunnelToken>,
) -> Result<EditedTunnel, LinkingError> {
    let linking_on = document
        .get("native")
        .filter(|native| native.is_object())
        .is_some_and(|native| native.get("enabled") != Some(&Value::Bool(false)));
    let saved = saved_section(&document)?;
    let mode = request
        .mode
        .or(saved.as_ref().map(|section| section.mode.into()))
        .unwrap_or(TunnelMode::Quick);
    let hostname = request
        .hostname
        .clone()
        .or_else(|| saved.as_ref().and_then(|section| section.hostname.clone()));
    let ingress = saved
        .as_ref()
        .and_then(|section| section.ingress)
        .unwrap_or(DEFAULT_TUNNEL_INGRESS);
    let token = request.token.clone().or_else(|| saved_token.clone());
    let setting = if request.enabled {
        if !linking_on {
            return Err(LinkingError::LinkingOff);
        }
        let tunnel = match (mode, hostname, token) {
            (TunnelMode::Quick, _, _) => CloudflareTunnel::Quick,
            (TunnelMode::Named, Some(address), Some(token)) => {
                CloudflareTunnel::Named { address, token }
            }
            (TunnelMode::Named, _, _) => return Err(LinkingError::TunnelIncomplete),
        };
        Some(TunnelSetting {
            ingress,
            provider: TunnelProvider::Cloudflare(tunnel),
        })
    } else {
        None
    };
    let existing = document
        .get("native")
        .and_then(|native| native.get("tunnel"))
        .filter(|tunnel| !tunnel.is_null())
        .cloned();
    let mut section = existing
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if request.enabled {
        section.remove("enabled");
    } else {
        section.insert("enabled".into(), Value::Bool(false));
    }
    section.insert("provider".into(), Value::String("cloudflare".into()));
    section.insert("mode".into(), Value::String(mode.as_str().into()));
    if let Some(hostname) = &request.hostname {
        section.insert("hostname".into(), Value::String(hostname.as_str().into()));
    }
    let section = Value::Object(section);
    // Off with no section is off already: nothing to keep.
    let publish = existing.as_ref() != Some(&section) && (request.enabled || existing.is_some());
    if publish {
        if let Some(native) = document.get_mut("native").and_then(Value::as_object_mut) {
            native.insert("tunnel".into(), section);
        }
    }
    Ok(EditedTunnel {
        document: publish.then_some(document),
        setting,
        mode,
        token: request
            .token
            .clone()
            .filter(|token| Some(token) != saved_token.as_ref()),
    })
}

/// `document` with `native` set to `enabled`: `None` to publish when it
/// already says so.
fn edit(mut document: Map<String, Value>, enabled: bool) -> Edited {
    let native = document
        .get_mut("native")
        .filter(|native| !native.is_null());
    let saved_on = native
        .as_ref()
        .is_some_and(|native| native.get("enabled") != Some(&Value::Bool(false)));
    // Settings' old loopback default, on its way back on: listen on the
    // network instead. An address the person wrote is theirs and is kept.
    let mut native = native;
    let mut moved = None;
    if let Some(section) = native
        .as_deref_mut()
        .and_then(Value::as_object_mut)
        .filter(|_| enabled)
    {
        let old = section
            .get("listenAddress")
            .and_then(Value::as_str)
            .and_then(|address| address.parse::<SocketAddr>().ok());
        if old == Some(SUPERSEDED_NATIVE_LISTEN) {
            section.insert(
                "listenAddress".into(),
                Value::String(DEFAULT_NATIVE_LISTEN.to_string()),
            );
            moved = Some(ListenMoved {
                from: SUPERSEDED_NATIVE_LISTEN,
                to: DEFAULT_NATIVE_LISTEN,
            });
        }
    }
    let changed = saved_on != enabled || moved.is_some();
    if changed {
        match (native, enabled) {
            (Some(Value::Object(section)), true) => {
                section.remove("enabled");
            }
            (Some(Value::Object(section)), false) => {
                section.insert("enabled".into(), Value::Bool(false));
            }
            (_, false) => {}
            (_, true) => {
                document.insert(
                    "native".into(),
                    serde_json::json!({ "listenAddress": DEFAULT_NATIVE_LISTEN.to_string() }),
                );
            }
        }
    }
    // The file passed the runtime configuration's check, so an address it
    // names parses; the publish checks the whole result again.
    let listen = enabled
        .then(|| {
            document
                .get("native")?
                .get("listenAddress")?
                .as_str()?
                .parse()
                .ok()
        })
        .flatten();
    // Off with no section is off already: nothing to keep.
    let publish = changed && (enabled || document.get("native").is_some_and(|n| !n.is_null()));
    Edited {
        document: publish.then_some(document),
        listen,
        moved,
    }
}

/// Whether this machine can listen at `address` now. Released at once: the
/// gateway that starts next binds it.
async fn bindable(address: SocketAddr) -> Result<(), LinkingError> {
    match tokio::net::TcpListener::bind(address).await {
        Ok(_) => Ok(()),
        Err(error) => {
            tracing::warn!(%address, %error, "native linking was not turned on: its address cannot be bound");
            Err(LinkingError::AddressInUse)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::runtime_config::{RuntimeConfig, MAX_CONFIG_BYTES};
    use crate::device_pairing::application::LinkingAuditUnavailable;
    use crate::mcp_servers::application::StoreLock;
    use crate::mcp_servers::infrastructure::{ConfigCheck, ConfigFiles};
    use nessa_sdk::infrastructure::clock::RuntimeClock;
    use std::sync::Mutex;

    /// `config.json` in memory; `held` keeps its lock taken.
    #[derive(Default)]
    struct Files {
        bytes: Mutex<Option<Vec<u8>>>,
        held: bool,
    }
    impl ConfigFiles for Files {
        fn read(&self, _limit: usize) -> std::io::Result<Option<Vec<u8>>> {
            Ok(self.bytes.lock().unwrap().clone())
        }
        fn publish(&self, bytes: &[u8]) -> std::io::Result<Published> {
            *self.bytes.lock().unwrap() = Some(bytes.to_vec());
            Ok(Published::Durable)
        }
        fn try_lock(&self) -> std::io::Result<Option<StoreLock>> {
            Ok((!self.held).then(|| Box::new(()) as StoreLock))
        }
    }

    /// The records kept; a phase named in `refuse` is not kept.
    #[derive(Default)]
    struct Audit {
        records: Mutex<Vec<LinkingAuditPhase>>,
        refuse: Option<fn(&LinkingAuditPhase) -> bool>,
    }
    impl LinkingAudit for Audit {
        fn record(&self, record: &LinkingAuditRecord) -> Result<(), LinkingAuditUnavailable> {
            if self.refuse.is_some_and(|refuse| refuse(&record.phase)) {
                return Err(LinkingAuditUnavailable);
            }
            assert_eq!(record.caller, caller());
            self.records.lock().unwrap().push(record.phase);
            Ok(())
        }
    }

    fn caller() -> LinkingCaller {
        LinkingCaller {
            organization_id: "org".into(),
            principal_id: "owner".into(),
            credential_id: "panel".into(),
        }
    }

    fn files(json: Option<&str>) -> Arc<Files> {
        Arc::new(Files {
            bytes: Mutex::new(json.map(|text| text.as_bytes().to_vec())),
            held: false,
        })
    }

    fn native(files: &Files) -> Option<Value> {
        let bytes = files.bytes.lock().unwrap().clone()?;
        let document: Value = serde_json::from_slice(&bytes).unwrap();
        document.get("native").cloned()
    }

    struct Linking {
        linking: ConfigLinking,
        restart: Arc<SelfRestart>,
        audit: Arc<Audit>,
    }

    fn linking_with(files: Arc<Files>, running: bool, audit: Audit) -> Linking {
        let restart = Arc::new(SelfRestart::default());
        let audit = Arc::new(audit);
        let config = Arc::new(ConfigJson::new(
            files,
            ConfigCheck {
                limit: MAX_CONFIG_BYTES,
                parses: Box::new(|bytes| RuntimeConfig::parse(bytes).is_ok()),
            },
            Arc::new(RuntimeClock::new()),
        ));
        Linking {
            linking: ConfigLinking::new(config, audit.clone(), running, restart.clone()),
            restart,
            audit,
        }
    }

    fn linking(files: Arc<Files>, running: bool) -> Linking {
        linking_with(files, running, Audit::default())
    }

    impl Linking {
        async fn set(&self, enabled: bool) -> Result<LinkingChange, LinkingError> {
            self.linking.set(caller(), enabled).await
        }
        fn records(&self) -> Vec<LinkingAuditPhase> {
            self.audit.records.lock().unwrap().clone()
        }
    }

    /// A port nothing listens on, for the bind check to find free.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn off_at(address: &str) -> String {
        format!(r#"{{"native":{{"enabled":false,"listenAddress":"{address}"}}}}"#)
    }

    #[tokio::test]
    async fn on_names_the_default_address_and_keeps_the_rest() {
        let files = files(Some(r#"{"session":{"handshakeTimeoutMs":1000}}"#));
        // A run that listens already holds its address, so nothing is bound
        // here: the edit is what is checked, whoever has the default port.
        let linking = linking(files.clone(), true);
        assert_eq!(
            linking.set(true).await,
            Ok(LinkingChange {
                enabled: true,
                applies: LinkingApply::Unchanged
            })
        );
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": "0.0.0.0:47650" }))
        );
        let bytes = files.bytes.lock().unwrap().clone().unwrap();
        assert!(RuntimeConfig::parse(&bytes).unwrap().session().is_ok());
        assert_eq!(
            linking.records(),
            [
                LinkingAuditPhase::Requested,
                LinkingAuditPhase::Applied {
                    applies: LinkingApply::Unchanged,
                    written: true,
                    moved: None
                }
            ]
        );
    }

    #[tokio::test]
    async fn turning_on_restarts_once_the_answer_had_its_grace() {
        let address = format!("127.0.0.1:{}", free_port());
        let files = files(Some(&off_at(&address)));
        let linking = linking(files.clone(), false);
        let started = tokio::time::Instant::now();
        assert_eq!(
            linking.set(true).await.unwrap().applies,
            LinkingApply::Restarting
        );
        assert!(!linking.restart.restarts(), "held until the answer is out");
        tokio::time::timeout(Duration::from_secs(2), linking.restart.requested())
            .await
            .expect("the restart is asked for after the answer's grace");
        assert!(started.elapsed() >= ANSWER_GRACE);
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": address }))
        );
        assert_eq!(
            linking.records(),
            [
                LinkingAuditPhase::Requested,
                LinkingAuditPhase::Applied {
                    applies: LinkingApply::Restarting,
                    written: true,
                    moved: None
                }
            ]
        );
    }

    #[tokio::test]
    async fn off_keeps_the_addresses_and_on_finds_them_again() {
        let address = format!("127.0.0.1:{}", free_port());
        let files = files(Some(&format!(
            r#"{{"native":{{"listenAddress":"{address}","relay":"192.0.2.1:9"}}}}"#
        )));
        let running = linking(files.clone(), true);
        assert_eq!(
            running.set(false).await.unwrap().applies,
            LinkingApply::Restarting
        );
        assert_eq!(
            native(&files),
            Some(serde_json::json!({
                "enabled": false, "listenAddress": address, "relay": "192.0.2.1:9"
            }))
        );
        let bytes = files.bytes.lock().unwrap().clone().unwrap();
        assert!(RuntimeConfig::parse(&bytes).unwrap().native.is_none());
        let stopped = linking(files.clone(), false);
        assert_eq!(
            stopped.set(true).await.unwrap().applies,
            LinkingApply::Restarting
        );
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": address, "relay": "192.0.2.1:9" }))
        );
    }

    #[tokio::test]
    async fn on_replaces_the_old_loopback_default_and_keeps_any_other_address() {
        // Settings named 127.0.0.1:47650 before it listened on the network.
        // A run serving there that is asked on again moves to the network
        // address, which applies only by starting again, and the audit says
        // what moved. A run that listens holds its address, so nothing is
        // bound here.
        let files = files(Some(r#"{"native":{"listenAddress":"127.0.0.1:47650"}}"#));
        let linking = linking(files.clone(), true);
        assert_eq!(
            linking.set(true).await.unwrap().applies,
            LinkingApply::Restarting
        );
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": "0.0.0.0:47650" }))
        );
        assert_eq!(
            linking.records().last(),
            Some(&LinkingAuditPhase::Applied {
                applies: LinkingApply::Restarting,
                written: true,
                moved: Some(ListenMoved {
                    from: SUPERSEDED_NATIVE_LISTEN,
                    to: DEFAULT_NATIVE_LISTEN,
                }),
            })
        );
        // Another loopback port is the person's own choice.
        let files = self::files(Some(&off_at("127.0.0.1:47651")));
        let linking = self::linking(files.clone(), true);
        assert_eq!(
            linking.set(true).await.unwrap().applies,
            LinkingApply::Unchanged
        );
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": "127.0.0.1:47651" }))
        );
    }

    #[tokio::test]
    async fn a_value_the_running_gateway_has_restarts_nothing() {
        let files = files(None);
        let linking = linking(files.clone(), false);
        assert_eq!(
            linking.set(false).await.unwrap(),
            LinkingChange {
                enabled: false,
                applies: LinkingApply::Unchanged
            }
        );
        assert_eq!(*files.bytes.lock().unwrap(), None, "nothing was written");
        tokio::time::sleep(ANSWER_GRACE * 2).await;
        assert!(linking.restart.serving(), "nothing was scheduled");
    }

    #[tokio::test]
    async fn a_change_while_a_restart_is_held_or_the_gateway_stops_is_busy() {
        let address = format!("127.0.0.1:{}", free_port());
        let files = files(Some(&off_at(&address)));
        let linking = linking(files.clone(), false);
        assert_eq!(
            linking.set(true).await.unwrap().applies,
            LinkingApply::Restarting
        );
        // Off now would be unchanged against this run, and the restart
        // already held would bring linking back off: it waits instead.
        assert_eq!(linking.set(false).await, Err(LinkingError::Busy));
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": address }))
        );
        assert_eq!(
            linking.records().last(),
            Some(&LinkingAuditPhase::Refused(LinkingError::Busy))
        );

        let stopping = linking_with(files.clone(), true, Audit::default());
        stopping.restart.stop();
        assert_eq!(stopping.set(false).await, Err(LinkingError::Busy));
        assert!(native(&files).unwrap().get("enabled").is_none());
    }

    #[tokio::test]
    async fn an_address_this_machine_cannot_bind_saves_nothing() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let written = off_at(&taken.local_addr().unwrap().to_string());
        let files = files(Some(&written));
        let linking = linking(files.clone(), false);
        assert_eq!(linking.set(true).await, Err(LinkingError::AddressInUse));
        assert_eq!(
            files.bytes.lock().unwrap().as_deref(),
            Some(written.as_bytes())
        );
        assert!(linking.restart.serving());
        assert_eq!(
            linking.records(),
            [
                LinkingAuditPhase::Requested,
                LinkingAuditPhase::Refused(LinkingError::AddressInUse)
            ]
        );
    }

    #[tokio::test]
    async fn a_file_that_does_not_parse_or_a_held_lock_is_refused() {
        let broken = files(Some(r#"{"native":{"listenAddress":"localhost:1"}}"#));
        let linking_broken = linking(broken.clone(), false);
        assert_eq!(
            linking_broken.set(true).await,
            Err(LinkingError::Unavailable)
        );
        assert_eq!(
            linking_broken.records().last(),
            Some(&LinkingAuditPhase::Refused(LinkingError::Unavailable))
        );
        let held = Arc::new(Files {
            bytes: Mutex::new(None),
            held: true,
        });
        let linking_held = linking(held, false);
        assert_eq!(linking_held.set(true).await, Err(LinkingError::Busy));
        assert_eq!(
            linking_held.records().last(),
            Some(&LinkingAuditPhase::Refused(LinkingError::Busy))
        );
    }

    #[tokio::test]
    async fn a_requested_record_not_kept_changes_nothing() {
        let address = format!("127.0.0.1:{}", free_port());
        let written = off_at(&address);
        let files = files(Some(&written));
        let linking = linking_with(
            files.clone(),
            false,
            Audit {
                refuse: Some(|phase| *phase == LinkingAuditPhase::Requested),
                ..Audit::default()
            },
        );
        assert_eq!(linking.set(true).await, Err(LinkingError::AuditUnavailable));
        assert_eq!(
            files.bytes.lock().unwrap().as_deref(),
            Some(written.as_bytes())
        );
        assert!(linking.restart.serving());
    }

    #[tokio::test]
    async fn an_outcome_not_kept_leaves_the_change_for_the_next_start() {
        let address = format!("127.0.0.1:{}", free_port());
        let files = files(Some(&off_at(&address)));
        let linking = linking_with(
            files.clone(),
            false,
            Audit {
                refuse: Some(|phase| matches!(phase, LinkingAuditPhase::Applied { .. })),
                ..Audit::default()
            },
        );
        assert_eq!(linking.set(true).await, Err(LinkingError::AuditUnavailable));
        assert_eq!(
            native(&files),
            Some(serde_json::json!({ "listenAddress": address }))
        );
        tokio::time::sleep(ANSWER_GRACE * 2).await;
        assert!(linking.restart.serving(), "nothing restarts unrecorded");
    }

    #[tokio::test]
    async fn the_restart_s_end_is_recorded_with_the_change() {
        let address = format!("127.0.0.1:{}", free_port());
        let files = files(Some(&off_at(&address)));
        let linking = linking(files, false);
        linking.set(true).await.unwrap();
        linking.restart.finish(RestartEnd::Exec);
        assert_eq!(
            linking.records().last(),
            Some(&LinkingAuditPhase::Restart(LinkingRestart::Exec))
        );
    }

    #[tokio::test]
    async fn a_file_already_on_is_still_bound_before_a_restart() {
        // Saved on by a change whose outcome was not kept, or by hand, while
        // this run does not listen: the restart would fail on a taken port.
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let written = format!(
            r#"{{"native":{{"listenAddress":"{}"}}}}"#,
            taken.local_addr().unwrap()
        );
        let files = files(Some(&written));
        let linking = linking(files.clone(), false);
        assert_eq!(linking.set(true).await, Err(LinkingError::AddressInUse));
        assert!(linking.restart.serving());
        drop(taken);
        let linking = super::tests::linking(files, false);
        assert_eq!(
            linking.set(true).await,
            Ok(LinkingChange {
                enabled: true,
                applies: LinkingApply::Restarting
            })
        );
        assert_eq!(
            linking.records().last(),
            Some(&LinkingAuditPhase::Applied {
                applies: LinkingApply::Restarting,
                written: false,
                moved: None
            })
        );
    }

    /// A linking that changes the tunnel too, its token kept beneath
    /// `namespace`, compared with `running`.
    fn tunnel_linking(
        files: Arc<Files>,
        namespace: &std::path::Path,
        running: Option<TunnelSetting>,
    ) -> Linking {
        let Linking {
            linking,
            restart,
            audit,
        } = linking(files, true);
        Linking {
            linking: linking.with_tunnel(TunnelFiles::new(namespace), running),
            restart,
            audit,
        }
    }

    fn request(enabled: bool, mode: Option<TunnelMode>) -> TunnelRequest {
        TunnelRequest {
            enabled,
            mode,
            hostname: None,
            token: None,
        }
    }

    fn address(text: &str) -> nessa_protocol::tunnel::TunnelAddress {
        nessa_protocol::tunnel::TunnelAddress::try_from(text.to_owned()).unwrap()
    }

    /// Linking on, with the tunnel's ingress on a free port.
    fn linking_on_with_ingress() -> (String, SocketAddr) {
        let ingress: SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
        (
            format!(
                r#"{{"native":{{"listenAddress":"0.0.0.0:47650","tunnel":{{"enabled":false,"ingress":"{ingress}"}}}}}}"#
            ),
            ingress,
        )
    }

    fn tunnel_section(files: &Files) -> Value {
        native(files).unwrap()["tunnel"].clone()
    }

    #[tokio::test]
    async fn a_quick_tunnel_turned_on_is_saved_and_restarts() {
        let namespace = tempfile::tempdir().unwrap();
        let (written, ingress) = linking_on_with_ingress();
        let files = files(Some(&written));
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        assert_eq!(
            linking.linking.set_tunnel(caller(), request(true, None)).await,
            Ok(TunnelChange {
                enabled: true,
                mode: TunnelMode::Quick,
                applies: LinkingApply::Restarting
            })
        );
        assert_eq!(
            tunnel_section(&files),
            serde_json::json!({ "ingress": ingress.to_string(), "provider": "cloudflare", "mode": "quick" })
        );
        let bytes = files.bytes.lock().unwrap().clone().unwrap();
        assert!(RuntimeConfig::parse(&bytes).is_ok());
        assert!(!linking.restart.serving(), "a restart is held");
        assert_eq!(
            linking.records(),
            [
                LinkingAuditPhase::Requested,
                LinkingAuditPhase::Applied {
                    applies: LinkingApply::Restarting,
                    written: true,
                    moved: None
                }
            ]
        );
    }

    #[tokio::test]
    async fn the_tunnel_this_run_serves_saved_again_restarts_nothing() {
        let namespace = tempfile::tempdir().unwrap();
        let (written, ingress) = linking_on_with_ingress();
        let files = files(Some(&written));
        let running = TunnelSetting {
            ingress,
            provider: TunnelProvider::Cloudflare(CloudflareTunnel::Quick),
        };
        // This run binds the ingress already, so it is not tried.
        let _held = std::net::TcpListener::bind(ingress).unwrap();
        let linking = tunnel_linking(files.clone(), namespace.path(), Some(running));
        let change = linking
            .linking
            .set_tunnel(caller(), request(true, Some(TunnelMode::Quick)))
            .await
            .unwrap();
        assert_eq!(change.applies, LinkingApply::Unchanged);
        assert!(linking.restart.serving());
    }

    #[tokio::test]
    async fn the_tunnel_needs_linking_on() {
        let namespace = tempfile::tempdir().unwrap();
        let files = files(Some(&off_at("0.0.0.0:47650")));
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        assert_eq!(
            linking.linking.set_tunnel(caller(), request(true, None)).await,
            Err(LinkingError::LinkingOff)
        );
        assert_eq!(native(&files).unwrap().get("tunnel"), None, "nothing saved");
        assert_eq!(
            linking.records(),
            [
                LinkingAuditPhase::Requested,
                LinkingAuditPhase::Refused(LinkingError::LinkingOff)
            ]
        );
    }

    #[tokio::test]
    async fn a_named_tunnel_needs_a_hostname_and_a_token_and_keeps_its_token_private() {
        use std::os::unix::fs::PermissionsExt;
        let namespace = tempfile::tempdir().unwrap();
        let (written, _) = linking_on_with_ingress();
        let files = files(Some(&written));
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        let named = TunnelRequest {
            hostname: Some(address("https://gateway.example.com")),
            ..request(true, Some(TunnelMode::Named))
        };
        assert_eq!(
            linking.linking.set_tunnel(caller(), named.clone()).await,
            Err(LinkingError::TunnelIncomplete),
            "no token given or saved"
        );
        let token = TunnelToken::new("eyJ0b2tlbiI6InNlY3JldCJ9".into()).unwrap();
        let change = linking
            .linking
            .set_tunnel(
                caller(),
                TunnelRequest {
                    token: Some(token.clone()),
                    ..named
                },
            )
            .await
            .unwrap();
        assert_eq!(change.mode, TunnelMode::Named);
        let section = tunnel_section(&files);
        assert_eq!(section["hostname"], "https://gateway.example.com");
        let config = String::from_utf8(files.bytes.lock().unwrap().clone().unwrap()).unwrap();
        assert!(!config.contains("secret") && !config.contains(token.expose()));
        let kept = TunnelFiles::new(namespace.path());
        assert_eq!(kept.read_token().unwrap(), Some(token));
        let mode = std::fs::metadata(kept.token_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[tokio::test]
    async fn the_saved_tunnel_reads_back_without_its_token_and_off_keeps_it() {
        let namespace = tempfile::tempdir().unwrap();
        let (written, ingress) = linking_on_with_ingress();
        let files = files(Some(&written));
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        linking
            .linking
            .set_tunnel(
                caller(),
                TunnelRequest {
                    hostname: Some(address("https://gateway.example.com")),
                    token: Some(TunnelToken::new("token".into()).unwrap()),
                    ..request(true, Some(TunnelMode::Named))
                },
            )
            .await
            .unwrap();
        // A fresh run, after the restart.
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        linking
            .linking
            .set_tunnel(caller(), request(false, None))
            .await
            .unwrap();
        assert_eq!(
            linking.linking.tunnel().await,
            Ok(SavedTunnel {
                enabled: false,
                mode: TunnelMode::Named,
                hostname: Some(address("https://gateway.example.com")),
                has_token: true,
                ingress,
            })
        );
        assert_eq!(tunnel_section(&files)["enabled"], false);
    }

    #[tokio::test]
    async fn an_ingress_this_machine_cannot_bind_is_refused_before_saving() {
        let namespace = tempfile::tempdir().unwrap();
        let (written, ingress) = linking_on_with_ingress();
        let _taken = std::net::TcpListener::bind(ingress).unwrap();
        let files = files(Some(&written));
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        assert_eq!(
            linking.linking.set_tunnel(caller(), request(true, None)).await,
            Err(LinkingError::AddressInUse)
        );
        assert_eq!(tunnel_section(&files)["enabled"], false, "nothing saved");
    }

    #[tokio::test]
    async fn off_with_no_tunnel_saved_writes_nothing() {
        let namespace = tempfile::tempdir().unwrap();
        let files = files(Some(r#"{"native":{"listenAddress":"0.0.0.0:47650"}}"#));
        let linking = tunnel_linking(files.clone(), namespace.path(), None);
        let change = linking
            .linking
            .set_tunnel(caller(), request(false, None))
            .await
            .unwrap();
        assert_eq!(change.applies, LinkingApply::Unchanged);
        assert_eq!(native(&files).unwrap().get("tunnel"), None);
    }
}
