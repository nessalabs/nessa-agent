//! Ordinary lifetime settings in config.json, using its existing shared lock,
//! bound, validation and atomic publication. No credential read/write occurs.
use crate::{
    conversation::{
        domain::{VmLifetime, VmLifetimeScope, VmLifetimeSettings},
        infrastructure::BoatRuntimeConfig,
    },
    mcp_servers::infrastructure::{ConfigJson, Published},
    workspaces::{
        application::{
            RuntimeAudit, RuntimeAuditEvent, RuntimeCaller, RuntimeChange, RuntimePublication,
            RuntimeSettings,
        },
        domain::Failure,
    },
};
use async_trait::async_trait;
use serde_json::{Map, Value};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct ConfigBoatRuntimeSettings {
    pub(crate) config: Arc<ConfigJson>,
    pub(crate) audit: Arc<dyn RuntimeAudit>,
    pub(crate) next_id: Arc<dyn Fn() -> Uuid + Send + Sync>,
}
fn parsed(document: &Map<String, Value>) -> Result<BoatRuntimeConfig, Failure> {
    serde_json::from_value(
        document
            .get("boat")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(|_| Failure::Storage)
}
fn edit(
    document: &mut Map<String, Value>,
    scope: &VmLifetimeScope,
    choice: Option<VmLifetime>,
) -> Result<(), Failure> {
    let boat = document
        .entry("boat")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or(Failure::Storage)?;
    match scope {
        VmLifetimeScope::Global => {
            if let Some(lifetime) = choice {
                boat.insert(
                    "autoStopSeconds".into(),
                    serde_json::json!(lifetime.seconds()),
                );
            } else {
                boat.remove("autoStopSeconds");
            }
        }
        VmLifetimeScope::Channel(channel) => {
            let overrides = boat
                .entry("channelAutoStopSeconds")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .ok_or(Failure::Storage)?;
            if let Some(lifetime) = choice {
                overrides.insert(
                    channel.as_str().into(),
                    serde_json::json!(lifetime.seconds()),
                );
            } else {
                overrides.remove(channel.as_str());
            }
        }
    }
    Ok(())
}
impl ConfigBoatRuntimeSettings {
    async fn apply(
        &self,
        scope: VmLifetimeScope,
        choice: Option<VmLifetime>,
        caller: RuntimeCaller,
    ) -> Result<VmLifetimeSettings, Failure> {
        let lock = self.config.lock().await.map_err(|_| Failure::Storage)?;
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            let mut document = this.config.document().map_err(|_| Failure::Storage)?;
            let before = parsed(&document)?;
            let before = match &scope {
                VmLifetimeScope::Global => before.default_override(),
                VmLifetimeScope::Channel(channel) => before.channel_override(channel),
            };
            let target = choice;
            let change = RuntimeChange {
                id: (this.next_id)(),
                scope: scope.clone(),
                caller,
                before,
                requested: target,
            };
            this.audit
                .record(&RuntimeAuditEvent::Requested(change.clone()))?;
            if before == target {
                this.audit.record(&RuntimeAuditEvent::Outcome(
                    change,
                    RuntimePublication::Unchanged,
                ))?;
                return Ok(parsed(&document)?.snapshot(scope.channel()));
            }
            // Validate the edited section before publication as well as the
            // whole file's shared RuntimeConfig check inside ConfigJson.
            let edited = edit(&mut document, &scope, choice)
                .and_then(|()| parsed(&document))
                .map(|config| config.snapshot(scope.channel()));
            let after = match edited {
                Ok(after) => after,
                Err(error) => {
                    this.audit.record(&RuntimeAuditEvent::Outcome(
                        change,
                        RuntimePublication::Failed,
                    ))?;
                    return Err(error);
                }
            };
            let published = catch_unwind(AssertUnwindSafe(|| this.config.publish(document)));
            let outcome = match published {
                Ok(Ok(Published::Durable)) => RuntimePublication::Durable,
                Ok(Ok(Published::NotDurable)) => RuntimePublication::NotDurable,
                Ok(Err(_)) => RuntimePublication::Failed,
                Err(_) => RuntimePublication::Uncertain,
            };
            this.audit
                .record(&RuntimeAuditEvent::Outcome(change, outcome))?;
            if outcome == RuntimePublication::Durable {
                Ok(after)
            } else {
                Err(Failure::Storage)
            }
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
}
#[async_trait]
impl RuntimeSettings for ConfigBoatRuntimeSettings {
    async fn read(&self, scope: &VmLifetimeScope) -> Result<VmLifetimeSettings, Failure> {
        let config = self.config.clone();
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || {
            let document = config.document().map_err(|_| Failure::Storage)?;
            Ok(parsed(&document)?.snapshot(scope.channel()))
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(
        &self,
        scope: VmLifetimeScope,
        lifetime: Option<VmLifetime>,
        caller: RuntimeCaller,
    ) -> Result<VmLifetimeSettings, Failure> {
        let this = self.clone();
        // Accepted work retains the shared lock through publication and its
        // outcome even if the caller drops the join waiter.
        tokio::spawn(async move { this.apply(scope, lifetime, caller).await })
            .await
            .map_err(|_| Failure::Storage)?
    }
}
