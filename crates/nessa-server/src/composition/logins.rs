//! The saved logins (issue #1260): one entry per agent and the device
//! switches in the gateway's vault, the Cedar policy over who may be handed
//! them, and their audit, composed once and shared by the logins methods
//! and every environment that starts agents on another machine.
use std::{collections::BTreeMap, path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::core::RunError;
use crate::logins::infrastructure::login_store;
use crate::logins::{
    application::SavedLoginsService,
    domain::devices_entry,
    infrastructure::{cedar::CedarLoginPolicyEvaluator, DurableLoginAudit},
};
use crate::secrets::{SecretStore, VaultSecret};
use nessa_protocol::agents::AgentId;
use nessa_vault::application::Vault;

/// The saved logins in `vault`, audited under `namespace`.
///
/// # Errors
/// The policy bundle does not validate.
pub(super) fn compose(
    namespace: &Path,
    vault: Arc<Vault>,
    changed: Arc<dyn Fn(AgentId) + Send + Sync>,
) -> Result<Arc<SavedLoginsService>, RunError> {
    let policy = CedarLoginPolicyEvaluator::new()
        .map_err(|_| RunError::Agent("the saved-login policy bundle does not validate".into()))?;
    let logins: BTreeMap<AgentId, Arc<dyn SecretStore>> = AgentId::ALL
        .iter()
        .map(|agent| (*agent, login_store(&vault, *agent)))
        .collect();
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
    ) -> crate::conversation::application::EnvironmentFuture<'a, Option<nessa_protocol::lease::Login>>
    {
        let service = self.0.clone();
        let environment = environment.clone();
        Box::pin(async move {
            // The vault read and the audit write block: off the executor.
            tokio::task::spawn_blocking(move || service.login_for(&environment, agent))
                .await
                .ok()
                .flatten()
        })
    }
}
