//! Where the pool's preferences are read and saved: the Nessa default and
//! each channel's override (`PoolPreference`), with when a computer is too
//! busy for new work. Implemented over config.json in infrastructure
//! (`ConfigPoolSettings`), under the file's one shared lock.
#![cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
use crate::conversation::domain::{PoolPreference, PoolSettings, SettingsScope};
use async_trait::async_trait;
use nessa_auth::domain::{OrganizationId, PrincipalId};

/// Why a pool preference could not be read or saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PoolSettingsError {
    /// The configuration could not be read or written just now.
    Unavailable,
}

/// Who asked for a change, as its audit record names them.
#[derive(Clone, Debug)]
pub(crate) struct PoolCaller {
    pub(crate) organization: OrganizationId,
    pub(crate) principal: PrincipalId,
    pub(crate) operation: String,
    pub(crate) credential: String,
}

#[async_trait]
pub(crate) trait PoolSettingsStore: Send + Sync {
    /// The default, `scope`'s override and the busy limits, from one read of
    /// the configuration.
    async fn read(&self, scope: &SettingsScope) -> Result<PoolSettings, PoolSettingsError>;
    /// Save `choice` for `scope`: the default's order and left-out list, or a
    /// channel's own preference, which `None` removes so the channel inherits
    /// again. Answers what one read after the save found. A save whose
    /// publication is not known to be durable is an error, never a success.
    async fn save(
        &self,
        scope: SettingsScope,
        choice: Option<PoolPreference>,
        caller: PoolCaller,
    ) -> Result<PoolSettings, PoolSettingsError>;
}
