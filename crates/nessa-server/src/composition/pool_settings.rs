//! The pool's preferences in config.json (`PoolConfig`), read and written
//! through the file's one shared lock, check and atomic publication. Each
//! save leaves two private records, as Boat's runtime settings do: what was
//! asked, before the file is touched, and how its publication ended, joined
//! by one operation id.
use crate::{
    conversation::{
        application::{PoolCaller, PoolSettingsError, PoolSettingsStore},
        domain::{PoolPreference, PoolSettings, SettingsScope},
        infrastructure::pool_config::{edit, PoolConfig},
    },
    mcp_servers::infrastructure::{ConfigJson, Published},
};
use async_trait::async_trait;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Map, Value};
use std::{io::Write, path::PathBuf, sync::Arc};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct ConfigPoolSettings {
    config: Arc<ConfigJson>,
    audit: PathBuf,
    clock: Arc<dyn Clock>,
    next_id: Arc<dyn Fn() -> Uuid + Send + Sync>,
}

impl ConfigPoolSettings {
    /// Over `config`, keeping each save's records in `audit`, created here.
    pub fn new(
        config: Arc<ConfigJson>,
        audit: PathBuf,
        clock: Arc<dyn Clock>,
        next_id: Arc<dyn Fn() -> Uuid + Send + Sync>,
    ) -> std::io::Result<Self> {
        create_directory(&audit)?;
        Ok(Self {
            config,
            audit,
            clock,
            next_id,
        })
    }

    fn parsed(document: &Map<String, Value>) -> Result<PoolConfig, PoolSettingsError> {
        PoolConfig::from_document(document).map_err(|_| PoolSettingsError::Unavailable)
    }

    fn record(
        &self,
        id: Uuid,
        phase: &str,
        scope: &SettingsScope,
        caller: &PoolCaller,
        detail: Value,
    ) -> Result<(), PoolSettingsError> {
        let unavailable = |_| PoolSettingsError::Unavailable;
        let target = match scope {
            SettingsScope::Global => json!({"scope":"global"}),
            SettingsScope::Channel(channel) => {
                json!({"scope":"channel","channelId":channel.as_str()})
            }
        };
        let record = json!({
            "kind": "pool_settings", "operationId": id.to_string(), "phase": phase,
            "observedAtMs": self.clock.unix_milliseconds(), "target": target, "detail": detail,
            "initiator": {
                "kind": "caller", "organizationId": caller.organization.as_str(),
                "principalId": caller.principal.as_str(), "requestId": caller.operation,
                "credentialId": caller.credential,
            },
        });
        let mut file = PrivateTempFile::new_in(&self.audit).map_err(unavailable)?;
        serde_json::to_writer(file.as_file_mut(), &record)
            .map_err(|_| PoolSettingsError::Unavailable)?;
        file.as_file_mut().write_all(b"\n").map_err(unavailable)?;
        file.as_file_mut().sync_all().map_err(unavailable)?;
        file.publish(&self.audit.join(format!("{id}-{phase}.json")))
            .map_err(unavailable)?;
        sync_directory(&self.audit).map_err(unavailable)
    }

    async fn apply(
        &self,
        scope: SettingsScope,
        choice: Option<PoolPreference>,
        caller: PoolCaller,
    ) -> Result<PoolSettings, PoolSettingsError> {
        let lock = self
            .config
            .lock()
            .await
            .map_err(|_| PoolSettingsError::Unavailable)?;
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            let mut document = this
                .config
                .document()
                .map_err(|_| PoolSettingsError::Unavailable)?;
            let before = Self::parsed(&document)?;
            let id = (this.next_id)();
            let asked = json!({
                "before": before.chosen(&scope).as_ref().map(preference),
                "requested": choice.as_ref().map(preference),
            });
            this.record(id, "requested", &scope, &caller, asked)?;
            if before.chosen(&scope) == choice
                || (scope == SettingsScope::Global
                    && choice.is_none()
                    && before.chosen(&scope) == Some(PoolPreference::default()))
            {
                this.record(
                    id,
                    "outcome",
                    &scope,
                    &caller,
                    json!({"outcome":"unchanged"}),
                )?;
                return Ok(before.snapshot(&scope));
            }
            edit(&mut document, &scope, choice.as_ref())
                .map_err(|()| PoolSettingsError::Unavailable)?;
            let after = Self::parsed(&document)?.snapshot(&scope);
            let outcome = match this.config.publish(document) {
                Ok(Published::Durable) => "durable",
                Ok(Published::NotDurable) => "not_durable",
                Err(_) => "failed",
            };
            this.record(id, "outcome", &scope, &caller, json!({"outcome":outcome}))?;
            if outcome == "durable" {
                Ok(after)
            } else {
                Err(PoolSettingsError::Unavailable)
            }
        })
        .await
        .map_err(|_| PoolSettingsError::Unavailable)?
    }
}

fn preference(preference: &PoolPreference) -> Value {
    let names = |peers: &[nessa_sdk::domain::agent_execution::leases::PeerEnvironment]| {
        peers.iter().map(|peer| peer.name()).collect::<Vec<_>>()
    };
    json!({"order": names(preference.order()), "excluded": names(preference.excluded())})
}

#[async_trait]
impl PoolSettingsStore for ConfigPoolSettings {
    async fn read(&self, scope: &SettingsScope) -> Result<PoolSettings, PoolSettingsError> {
        let config = self.config.clone();
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || {
            let document = config
                .document()
                .map_err(|_| PoolSettingsError::Unavailable)?;
            Ok(Self::parsed(&document)?.snapshot(&scope))
        })
        .await
        .map_err(|_| PoolSettingsError::Unavailable)?
    }

    async fn save(
        &self,
        scope: SettingsScope,
        choice: Option<PoolPreference>,
        caller: PoolCaller,
    ) -> Result<PoolSettings, PoolSettingsError> {
        let this = self.clone();
        // An accepted save keeps the lock through publication and its record
        // even when its caller stops waiting.
        tokio::spawn(async move { this.apply(scope, choice, caller).await })
            .await
            .map_err(|_| PoolSettingsError::Unavailable)?
    }
}
