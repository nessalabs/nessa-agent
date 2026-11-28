//! Scoped app authentication reuses the credential owner and current-session
//! checks without creating an ordinary Product or browser session.

use super::{validate_current, AuthenticatedSession};
use crate::{
    application::{
        app_credential::{AppCredentialBinding, AppCredentialSnapshot, AppCredentials},
        origin::CredentialOrigin,
        ports::{AccessError, Clock, CredentialEvidence, Decision, PolicyEvaluator},
    },
    domain::{Action, AppCall, AudienceId, AuthContext, Resource, ResourceId},
};

/// Authenticated app identity. This is not an ordinary session and grants
/// no request or conversation operation.
#[derive(Clone, Debug)]
pub struct ScopedAppSession {
    identity: AuthenticatedSession,
    origin: CredentialOrigin,
    binding: AppCredentialBinding,
}

impl ScopedAppSession {
    /// Identity selectors stamped with the registry-proven app namespace.
    pub fn context(&self) -> &AuthContext {
        self.identity.context()
    }

    /// Exclusive expiry reported by the credential owner.
    pub fn expires_at(&self) -> Option<u64> {
        self.identity.expires_at()
    }

    /// Auth revision at entry, not proof of a current permission.
    pub fn auth_revision(&self) -> u64 {
        self.identity.auth_revision()
    }

    /// The app and the capabilities approved at entry. Ask
    /// [`ReadCurrentAppSession`] before acting on them.
    pub fn binding(&self) -> &AppCredentialBinding {
        &self.binding
    }

    /// The app's own identity as a session, for the one dispatcher that
    /// serves an app: it checks [`ReadCurrentAppSession`] and the method's
    /// capability first, and acts only on conversations this principal
    /// owns. Never hand it to the ordinary Product dispatcher, which
    /// authorizes by grants this identity does not hold.
    pub fn app_identity(&self) -> &AuthenticatedSession {
        &self.identity
    }
}

/// Authenticate app proof for a trusted app composition, not a mounted route.
pub struct AuthenticateAppSession<'a> {
    pub credentials: &'a dyn AppCredentials,
    pub clock: &'a dyn Clock,
    pub policy: &'a dyn PolicyEvaluator,
}

impl AuthenticateAppSession<'_> {
    pub async fn execute(
        &self,
        evidence: &CredentialEvidence,
        audience: &AudienceId,
    ) -> Result<ScopedAppSession, AccessError> {
        let AppCredentialSnapshot { access, binding } =
            self.credentials.verify_app(evidence, audience).await?;
        let credential = &access.credential;
        if credential.audience_id() != audience {
            return Err(AccessError::IdentityMismatch);
        }
        let context = AuthContext::new(
            credential.principal_id().clone(),
            credential.organization_id().clone(),
            access.membership.id().clone(),
            credential.id().clone(),
            credential.audience_id().clone(),
        )
        .with_verified_app(binding.app().clone());
        let identity = AuthenticatedSession {
            context,
            expires_at: credential.expires_at(),
            auth_revision: access.revision,
        };
        validate_current(&identity, &access, self.clock)?;
        let action = Action::new("appCredential.enter").map_err(|_| AccessError::Unavailable)?;
        let gateway = Resource::new(
            credential.organization_id().clone(),
            ResourceId::new(audience.as_str()).map_err(|_| AccessError::Unavailable)?,
        );
        if self
            .policy
            .evaluate(identity.context(), &action, &gateway, &access)?
            != Decision::Allow
        {
            return Err(AccessError::Denied);
        }
        Ok(ScopedAppSession {
            identity,
            origin: access.origin,
            binding,
        })
    }
}

/// Current app identity and approved capabilities, for a caller to ask
/// policy about a separate operation. The binding is not a permission verdict.
pub struct ReadCurrentAppSession<'a> {
    pub credentials: &'a dyn AppCredentials,
    pub clock: &'a dyn Clock,
}

impl ReadCurrentAppSession<'_> {
    pub async fn execute(
        &self,
        session: &ScopedAppSession,
    ) -> Result<AppCredentialBinding, AccessError> {
        let AppCredentialSnapshot { access, binding } = self
            .credentials
            .read_app(session.context().credential_id())
            .await?;
        validate_current(&session.identity, &access, self.clock)?;
        if access.origin != session.origin || binding != session.binding {
            return Err(AccessError::IdentityMismatch);
        }
        Ok(binding)
    }
}

/// Whether an app session may call `method` now: its credential is still
/// current, and policy admits the method for the operations its owner
/// approved. `Denied` is a refusal of this one call; any other error ends the
/// session.
pub struct AdmitAppCall<'a> {
    pub credentials: &'a dyn AppCredentials,
    pub clock: &'a dyn Clock,
    pub policy: &'a dyn PolicyEvaluator,
}

impl AdmitAppCall<'_> {
    pub async fn execute(
        &self,
        session: &ScopedAppSession,
        method: &str,
    ) -> Result<AppCredentialBinding, AccessError> {
        let AppCredentialSnapshot { access, binding } = self
            .credentials
            .read_app(session.context().credential_id())
            .await?;
        validate_current(&session.identity, &access, self.clock)?;
        if access.origin != session.origin || binding != session.binding {
            return Err(AccessError::IdentityMismatch);
        }
        let action = Action::new("appCredential.call").map_err(|_| AccessError::Unavailable)?;
        let call = Resource::new(
            session.context().organization_id().clone(),
            ResourceId::new(session.context().audience_id().as_str())
                .map_err(|_| AccessError::Unavailable)?,
        )
        .with_app_call(AppCall {
            method: method.to_owned(),
            capabilities: binding
                .capabilities()
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect(),
        });
        match self
            .policy
            .evaluate(session.context(), &action, &call, &access)?
        {
            Decision::Allow => Ok(binding),
            _ => Err(AccessError::Denied),
        }
    }
}
