//! Ordinary default and channel runtime preferences. The config adapter owns
//! shared-file locking and publication; this port carries no Boat token.
use super::super::domain::Failure;
use crate::domain::{SettingsScope, VmLifetime, VmLifetimeSettings};
use async_trait::async_trait;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use uuid::Uuid;

/// Attribution of a settings command, supplied by the authenticated route.
#[derive(Clone)]
pub struct RuntimeCaller {
    pub organization: OrganizationId,
    pub principal: PrincipalId,
    pub operation: String,
    pub credential: String,
}
#[async_trait]
pub trait RuntimeSettings: Send + Sync {
    /// Default and override from a single validated document.
    async fn read(&self, scope: &SettingsScope) -> Result<VmLifetimeSettings, Failure>;
    /// Set an explicit lifetime, or reset a channel to inheritance. The
    /// accepted save owns its publication and attributed outcome after caller loss.
    /// A storage/audit refusal cannot be returned as successful publication.
    async fn save(
        &self,
        scope: SettingsScope,
        lifetime: Option<VmLifetime>,
        caller: RuntimeCaller,
    ) -> Result<VmLifetimeSettings, Failure>;
}

#[derive(Clone)]
pub struct RuntimeChange {
    pub id: Uuid,
    pub scope: SettingsScope,
    pub caller: RuntimeCaller,
    pub before: Option<VmLifetime>,
    pub requested: Option<VmLifetime>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePublication {
    Unchanged,
    Durable,
    NotDurable,
    Failed,
    Uncertain,
}
pub enum RuntimeAuditEvent {
    Requested(RuntimeChange),
    Outcome(RuntimeChange, RuntimePublication),
}
pub trait RuntimeAudit: Send + Sync {
    fn record(&self, event: &RuntimeAuditEvent) -> Result<(), Failure>;
}
