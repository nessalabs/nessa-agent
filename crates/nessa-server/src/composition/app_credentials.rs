//! Installed apps' own credentials, kept in the same local credential
//! registry as every other credential: issued through its app path
//! (`LocalCredentialStore::issue_app`, which asks Cedar
//! `appCredential.issue` of the owner), revoked through the credential
//! admin, and checked by the same store when an app signs in.
use std::sync::Arc;

use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::{LocalCredentialStore, LocalStoreError};
use nessa_auth::application::app_credential::{
    AppCapabilities, AppCredentials, IssueAppCredential,
};
use nessa_auth::application::credential_admin::{
    CredentialAdmin, CredentialAdminError, IssueCredentialOutcome, ListCredentialsRequest,
    RevokeCredentialRequest,
};
use nessa_auth::application::ports::{Clock, PortFuture};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_auth::domain::{CredentialId, PrincipalId};
use nessa_types::app_identity::ExtensionAppId;
use uuid::Uuid;

use super::local_auth::SystemClock;
use crate::installed_apps::application::{
    AppCredentialAuthority, CredentialRefusal, IssuedAppCredential,
};

/// How long an app's credential lasts. The owner issues a new one by
/// reinstalling, and uninstalling revokes it at once.
const APP_CREDENTIAL_LIFETIME_SECONDS: u64 = 365 * 24 * 60 * 60;

pub(super) struct LocalAppCredentials {
    pub(super) store: Arc<LocalCredentialStore>,
    pub(super) admin: Arc<dyn CredentialAdmin>,
    pub(super) policy: Arc<CedarPolicyEvaluator>,
}

impl LocalAppCredentials {
    /// The live credentials issued to `app` in `organization`.
    fn live(
        &self,
        organization: &str,
        app: &ExtensionAppId,
    ) -> Result<Vec<String>, CredentialRefusal> {
        let now = SystemClock.unix_seconds();
        let listed = self
            .store
            .list_sync(&ListCredentialsRequest {
                organization_id: organization.to_owned(),
            })
            .map_err(|_| CredentialRefusal::Unavailable)?;
        Ok(listed
            .into_iter()
            .filter(|metadata| metadata.revoked_at.is_none())
            .filter(|metadata| {
                CredentialId::new(&metadata.id)
                    .ok()
                    .and_then(|id| self.store.app_binding(&id, now).ok())
                    .is_some_and(|binding| binding.app() == app)
            })
            .map(|metadata| metadata.id)
            .collect())
    }

    /// Revoke every live credential of `app` under the command
    /// `request_id`. Issuing supersedes an app's older credential in the
    /// same commit, so it holds at most one, revoked under the command's own
    /// key: a retry of the command answers what it revoked (so the caller can
    /// still record it), and a late retry that finds a newer credential is a
    /// conflict, never a revoke of that one.
    async fn revoke_live(
        &self,
        organization: &str,
        issuer: &str,
        request_id: &str,
        app: &ExtensionAppId,
    ) -> Result<usize, CredentialRefusal> {
        let live = self.live(organization, app)?;
        if live.is_empty() {
            let issuer = PrincipalId::new(issuer).map_err(|_| CredentialRefusal::Unavailable)?;
            let earlier = self
                .store
                .revoked_under(&issuer, request_id)
                .map_err(|_| CredentialRefusal::Unavailable)?;
            return Ok(usize::from(earlier));
        }
        for (index, credential_id) in live.iter().enumerate() {
            // More than one is only left from before issuing superseded; each
            // after the first gets a key no client request id can be.
            let key = match index {
                0 => request_id.to_owned(),
                n => internal_key(&format!("extra-{n}"), request_id),
            };
            self.admin
                .revoke(RevokeCredentialRequest {
                    request_id: key,
                    issuer_principal_id: issuer.to_owned(),
                    credential_id: credential_id.clone(),
                    revoked_at: SystemClock.unix_seconds(),
                })
                .await
                .map_err(|error| match error {
                    CredentialAdminError::Conflict => CredentialRefusal::Conflict,
                    _ => CredentialRefusal::Unavailable,
                })?;
        }
        Ok(live.len())
    }
}

/// A command key derived from a client's `request_id` for a step it takes
/// on its own: a digest, so no request id a client sends can equal it.
fn internal_key(purpose: &str, request_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{purpose}\0{request_id}"));
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("nessa-app-{purpose}-{hex}")
}

impl AppCredentialAuthority for LocalAppCredentials {
    fn issue<'a>(
        &'a self,
        owner: &'a AuthenticatedSession,
        request_id: String,
        app: ExtensionAppId,
        capabilities: AppCapabilities,
    ) -> PortFuture<'a, IssuedAppCredential, CredentialRefusal> {
        Box::pin(async move {
            // The registry ends the app's older credential in the same
            // commit that issues this one, so a refused or replayed issue
            // leaves the working one as it was.
            let credential_id = CredentialId::new(format!("app-{}", Uuid::new_v4().simple()))
                .map_err(|_| CredentialRefusal::Unavailable)?;
            let expires_at = SystemClock.unix_seconds() + APP_CREDENTIAL_LIFETIME_SECONDS;
            let granted = capabilities.iter().collect::<Vec<_>>();
            let outcome = self
                .store
                .issue_app(
                    owner,
                    IssueAppCredential {
                        request_id,
                        credential_id,
                        app,
                        capabilities,
                        expires_at,
                    },
                    &SystemClock,
                    self.policy.as_ref(),
                )
                .await
                .map_err(|error| match error {
                    LocalStoreError::Conflict => CredentialRefusal::Conflict,
                    _ => CredentialRefusal::Unavailable,
                })?;
            let IssueCredentialOutcome::Issued {
                metadata, evidence, ..
            } = outcome
            else {
                return Err(CredentialRefusal::Conflict);
            };
            Ok(IssuedAppCredential {
                credential_id: metadata.id,
                secret: String::from_utf8(evidence.expose_bytes().to_vec())
                    .map_err(|_| CredentialRefusal::Unavailable)?,
                expires_at: metadata.expires_at.unwrap_or(expires_at),
                capabilities: granted,
            })
        })
    }

    fn revoke_all<'a>(
        &'a self,
        organization: String,
        issuer: String,
        request_id: String,
        app: ExtensionAppId,
    ) -> PortFuture<'a, usize, CredentialRefusal> {
        Box::pin(async move {
            self.revoke_live(&organization, &issuer, &request_id, &app)
                .await
        })
    }

    fn sessions(&self) -> &dyn AppCredentials {
        self.store.as_ref()
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/composition/app_credentials.rs"]
mod tests;
