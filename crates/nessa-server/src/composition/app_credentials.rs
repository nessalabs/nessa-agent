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
use nessa_auth::domain::CredentialId;
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

    async fn revoke_live(
        &self,
        organization: &str,
        issuer: &str,
        request_id: &str,
        app: &ExtensionAppId,
    ) -> Result<usize, CredentialRefusal> {
        let live = self.live(organization, app)?;
        for credential_id in &live {
            self.admin
                .revoke(RevokeCredentialRequest {
                    // One key per credential, so a retry replays each.
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
            self.revoke_live(
                context.organization_id().as_str(),
                context.principal_id().as_str(),
                &request_id,
                &app,
            )
            .await?;
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
            match outcome {
                IssueCredentialOutcome::Issued {
                    metadata, evidence, ..
                } => Ok(IssuedAppCredential {
                    credential_id: metadata.id,
                    secret: String::from_utf8(evidence.expose_bytes().to_vec())
                        .map_err(|_| CredentialRefusal::Unavailable)?,
                    expires_at: metadata.expires_at.unwrap_or(expires_at),
                    capabilities: granted,
                }),
                IssueCredentialOutcome::ExistingSecretUnavailable { .. } => {
                    Err(CredentialRefusal::Conflict)
                }
            }
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
