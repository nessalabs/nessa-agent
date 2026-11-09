//! The saved logins (issue #1260): one entry per agent and the device
//! switches in the gateway's vault, the Cedar policy over who may be handed
//! them, and their audit, composed once and shared by the logins methods
//! and every environment that starts agents on another machine.
use std::{collections::BTreeMap, path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::core::RunError;
use crate::logins::{
    application::SavedLoginsService,
    domain::devices_entry,
    infrastructure::{cedar::CedarLoginPolicyEvaluator, DurableLoginAudit},
};
use crate::secrets::{SecretStore, VaultSecret};
use nessa_protocol::agents::AgentId;
use nessa_vault::application::Vault;

/// The saved logins kept in `logins` (one store per agent, from
/// `login_stores`) with the switches in `vault`, audited under `namespace`.
///
/// # Errors
/// The policy bundle does not validate.
pub(super) fn compose(
    namespace: &Path,
    vault: Arc<Vault>,
    logins: BTreeMap<AgentId, Arc<dyn SecretStore>>,
    changed: Arc<dyn Fn(AgentId) + Send + Sync>,
) -> Result<Arc<SavedLoginsService>, RunError> {
    let policy = CedarLoginPolicyEvaluator::new()
        .map_err(|_| RunError::Agent("the saved-login policy bundle does not validate".into()))?;
    Ok(Arc::new(
        SavedLoginsService::new(
            logins,
            Arc::new(VaultSecret::new(vault, devices_entry())),
            Arc::new(policy),
            Arc::new(DurableLoginAudit::new(
                conversation_root(namespace).join("audit").join("logins"),
                Arc::new(SystemClock),
            )),
        )
        .on_change(changed),
    ))
}

/// The environments' seam over the service: which login a harness started
/// on a machine is handed.
#[cfg(unix)]
pub(super) struct ServiceLogins(pub(super) Arc<SavedLoginsService>);

#[cfg(unix)]
impl crate::conversation::infrastructure::ssh_environment::SavedLogins for ServiceLogins {
    fn login<'a>(
        &'a self,
        environment: &'a nessa_sdk::domain::agent_execution::leases::EnvironmentRef,
        agent: AgentId,
        lease: &'a str,
    ) -> crate::conversation::application::EnvironmentFuture<'a, Option<nessa_protocol::lease::Login>>
    {
        let service = self.0.clone();
        let environment = environment.clone();
        let lease = lease.to_owned();
        Box::pin(async move {
            // The vault read and the audit write block: off the executor.
            tokio::task::spawn_blocking(move || service.login_for(&environment, agent, &lease))
                .await
                .ok()
                .flatten()
        })
    }
}

/// Boat's Claude sign-in, sent to a VM when its serve starts: the saved
/// Claude login, through the same policy and audit as a lease's handoff, and
/// only when it is a `claude setup-token` token (an API key is handed by
/// each lease instead). The service is set once composed: Boat is composed
/// before it, since the service tells Boat when the login changes.
#[cfg(unix)]
pub(super) struct ServedClaudeSignIns(pub(super) Arc<std::sync::OnceLock<Arc<SavedLoginsService>>>);

/// What the `lease` field of a Boat serve's handoff record says: the sign-in
/// went to a VM's serve, under no one lease.
#[cfg(unix)]
pub(super) const BOAT_SERVE: &str = "boat-serve";

#[cfg(unix)]
#[async_trait::async_trait]
impl crate::workspaces::application::ClaudeSignIns for ServedClaudeSignIns {
    async fn read(
        &self,
    ) -> Result<Option<crate::workspaces::domain::ClaudeSignIn>, crate::workspaces::domain::Failure>
    {
        use crate::workspaces::domain::ClaudeSignIn;
        let Some(service) = self.0.get().cloned() else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || {
            service
                .login_for_if(
                    &nessa_sdk::domain::agent_execution::leases::EnvironmentRef::Boat,
                    AgentId::Claude,
                    BOAT_SERVE,
                    |login| ClaudeSignIn::new(login.expose().to_owned()).is_ok(),
                )
                .and_then(|login| ClaudeSignIn::new(login.expose().to_owned()).ok())
        })
        .await
        .map_err(|_| crate::workspaces::domain::Failure::Storage)
    }
}
