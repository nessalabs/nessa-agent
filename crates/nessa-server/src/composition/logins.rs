//! The saved logins (issue #1260): one entry per agent and the device
//! switches in the gateway's vault, the Cedar policy over who may be handed
//! them, and their audit, composed once and shared by the logins methods
//! and every environment that starts agents on another machine.
//!
//! Codex's ChatGPT accounts are composed with them
//! (`docs/design/chatgpt-sign-in.md`). `NESSA_CHATGPT_SIGN_IN` names a file
//! describing another sign-in in place of OpenAI's, so the stress check can
//! point a gateway at a fake one on this machine; what it may say is
//! [`ChatgptSignInConfig::parse`]'s.
use std::{collections::BTreeMap, path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::core::RunError;
use crate::logins::{
    application::SavedLoginsService,
    chatgpt::{
        application::ChatgptLogins,
        infrastructure::{ChatgptSignInConfig, LoopbackListener, OpenaiSignIn},
    },
    domain::{chatgpt_entry, devices_entry},
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
    let audit = Arc::new(DurableLoginAudit::new(
        conversation_root(namespace).join("audit").join("logins"),
        Arc::new(SystemClock),
    ));
    let mut service = SavedLoginsService::new(
        logins,
        Arc::new(VaultSecret::new(vault.clone(), devices_entry())),
        Arc::new(policy),
        audit.clone(),
    )
    .on_change(changed);
    if let Some(authority) = chatgpt_authority() {
        service = service.with_chatgpt(Arc::new(ChatgptLogins::new(
            Arc::new(VaultSecret::new(vault, chatgpt_entry())),
            Arc::new(authority),
            Arc::new(LoopbackListener::new()),
            audit,
            Arc::new(SystemClock),
        )));
    }
    Ok(Arc::new(service))
}

/// OpenAI's sign-in, or the one `NESSA_CHATGPT_SIGN_IN` describes; `None`
/// (ChatGPT sign-in off this run, the API key still used) when that file
/// cannot be read or no HTTPS client can be built.
fn chatgpt_authority() -> Option<OpenaiSignIn> {
    let config = match std::env::var_os("NESSA_CHATGPT_SIGN_IN") {
        None => ChatgptSignInConfig::released(),
        Some(path) => match std::fs::read(&path).map(|bytes| ChatgptSignInConfig::parse(&bytes)) {
            Ok(Ok(config)) => config,
            Ok(Err(problem)) => {
                tracing::error!(
                    ?problem,
                    "ChatGPT sign-in is off this run: NESSA_CHATGPT_SIGN_IN does not describe one"
                );
                return None;
            }
            Err(error) => {
                tracing::error!(%error, "ChatGPT sign-in is off this run: NESSA_CHATGPT_SIGN_IN could not be read");
                return None;
            }
        },
    };
    let authority = OpenaiSignIn::new(config);
    if authority.is_none() {
        tracing::error!("ChatGPT sign-in is off this run: no HTTPS client could be built");
    }
    authority
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
    ) -> crate::conversation::application::EnvironmentFuture<
        'a,
        Option<crate::conversation::infrastructure::ssh_environment::HandedLogin>,
    > {
        let service = self.0.clone();
        let environment = environment.clone();
        let lease = lease.to_owned();
        Box::pin(async move {
            service.hand(environment, agent, lease).await.map(|handed| {
                crate::conversation::infrastructure::ssh_environment::HandedLogin {
                    login: handed.login,
                    renew_by_ms: handed.renew_by_ms,
                }
            })
        })
    }
}
