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

    /// Revoke every live credential of `app` but `keep`, each under its own
    /// key `<request_id>:<credential id>`. A command that already revoked
    /// answers with what it revoked then and revokes nothing more, so a late
    /// retry cannot reach a credential issued after it.
    async fn revoke_live(
        &self,
        organization: &str,
        issuer: &str,
        request_id: &str,
        app: &ExtensionAppId,
        keep: Option<&str>,
    ) -> Result<usize, CredentialRefusal> {
        let issuer_id = PrincipalId::new(issuer).map_err(|_| CredentialRefusal::Unavailable)?;
        let earlier = self
            .store
            .revoked_under(&issuer_id, request_id)
            .map_err(|_| CredentialRefusal::Unavailable)?;
        if earlier > 0 {
            return Ok(earlier);
        }
        let live = self
            .live(organization, app)?
            .into_iter()
            .filter(|id| Some(id.as_str()) != keep)
            .collect::<Vec<_>>();
        for credential_id in &live {
            self.admin
                .revoke(RevokeCredentialRequest {
                    request_id: format!("{request_id}:{credential_id}"),
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

impl AppCredentialAuthority for LocalAppCredentials {
    fn issue<'a>(
        &'a self,
        owner: &'a AuthenticatedSession,
        request_id: String,
        app: ExtensionAppId,
        capabilities: AppCapabilities,
    ) -> PortFuture<'a, IssuedAppCredential, CredentialRefusal> {
        Box::pin(async move {
            let context = owner.context();
            let organization = context.organization_id().as_str().to_owned();
            let issuer = context.principal_id().as_str().to_owned();
            // Issue first: a refusal (a replayed request, a stale session,
            // a full registry) then leaves the app's working credential as
            // it was. Only once the new one exists are the others revoked.
            let credential_id = CredentialId::new(format!("app-{}", Uuid::new_v4().simple()))
                .map_err(|_| CredentialRefusal::Unavailable)?;
            let expires_at = SystemClock.unix_seconds() + APP_CREDENTIAL_LIFETIME_SECONDS;
            let granted = capabilities.iter().collect::<Vec<_>>();
            let outcome = self
                .store
                .issue_app(
                    owner,
                    IssueAppCredential {
                        request_id: request_id.clone(),
                        credential_id,
                        app: app.clone(),
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
            let issued = match String::from_utf8(evidence.expose_bytes().to_vec()) {
                Ok(secret) => self
                    .revoke_live(
                        &organization,
                        &issuer,
                        &request_id,
                        &app,
                        Some(&metadata.id),
                    )
                    .await
                    .map(|_| secret),
                Err(_) => Err(CredentialRefusal::Unavailable),
            };
            let secret = match issued {
                Ok(secret) => secret,
                Err(refusal) => {
                    // Nobody will hold the new secret: take it back, so the app
                    // keeps at most the one it had.
                    let _ = self
                        .admin
                        .revoke(RevokeCredentialRequest {
                            request_id: format!("{request_id}:undo:{}", metadata.id),
                            issuer_principal_id: issuer.clone(),
                            credential_id: metadata.id.clone(),
                            revoked_at: SystemClock.unix_seconds(),
                        })
                        .await;
                    return Err(refusal);
                }
            };
            Ok(IssuedAppCredential {
                credential_id: metadata.id,
                secret,
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
            self.revoke_live(&organization, &issuer, &request_id, &app, None)
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
