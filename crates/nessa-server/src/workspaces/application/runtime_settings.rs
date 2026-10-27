//! Ordinary default and channel runtime preferences. The config adapter owns
//! shared-file locking and publication; this port carries no Boat token.
use super::super::domain::Failure;
use crate::conversation::domain::{VmLifetime, VmLifetimeScope, VmLifetimeSettings};
use async_trait::async_trait;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use uuid::Uuid;

/// Attribution of a settings command, supplied by the authenticated route.
#[derive(Clone)]
pub(crate) struct RuntimeCaller {
    pub(crate) organization: OrganizationId,
    pub(crate) principal: PrincipalId,
    pub(crate) operation: String,
    pub(crate) credential: String,
}
#[async_trait]
pub(crate) trait RuntimeSettings: Send + Sync {
    /// Default and override from a single validated document.
    async fn read(&self, scope: &VmLifetimeScope) -> Result<VmLifetimeSettings, Failure>;
    /// Set an explicit lifetime, or reset a channel to inheritance. The
    /// accepted save owns its publication and attributed outcome after caller loss.
    /// A storage/audit refusal cannot be returned as successful publication.
    async fn save(
        &self,
        scope: VmLifetimeScope,
        lifetime: Option<VmLifetime>,
        caller: RuntimeCaller,
    ) -> Result<VmLifetimeSettings, Failure>;
}

#[derive(Clone)]
pub(crate) struct RuntimeChange {
    pub(crate) id: Uuid,
    pub(crate) scope: VmLifetimeScope,
    pub(crate) caller: RuntimeCaller,
    pub(crate) before: Option<VmLifetime>,
    pub(crate) requested: Option<VmLifetime>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimePublication {
    Unchanged,
    Durable,
    NotDurable,
    Failed,
    Uncertain,
}
pub(crate) enum RuntimeAuditEvent {
    Requested(RuntimeChange),
    Outcome(RuntimeChange, RuntimePublication),
}
pub(crate) trait RuntimeAudit: Send + Sync {
    fn record(&self, event: &RuntimeAuditEvent) -> Result<(), Failure>;
}
