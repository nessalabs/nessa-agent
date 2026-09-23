use crate::{
    browser_session::domain::value_objects::{
        BrowserSessionOrigin, BrowserSessionState, RemovalReason,
    },
    core::trusted_origin::is_trusted_origin_value,
};
use nessa_auth::application::ports::{
    AccessError, CredentialEvidence, PortFuture, SessionEvidence, SessionVerifier,
    VerifiedSessionCredential,
};
use nessa_auth::domain::CredentialId;

/// Bounded session persistence selected by server composition.
pub trait SessionStore: Send + Sync {
    /// Insert one session and return the exact active `prior` atomically replaced.
    fn insert<'a>(
        &'a self,
        id: String,
        session: BrowserSessionState,
        prior: Option<String>,
        now: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSessionState)>>;
    fn get<'a>(&'a self, id: String) -> PortFuture<'a, Option<BrowserSessionState>>;
    /// Remove a session with its typed cause and verified explicit initiator.
    /// Automatic causes carry `None`; explicit sign-out carries the credential
    /// verified by the application immediately before this command.
    fn remove<'a>(
        &'a self,
        id: String,
        now: u64,
        reason: RemovalReason,
        initiator: Option<CredentialId>,
    ) -> PortFuture<'a, ()>;
    /// Reclaim an undisclosed login and atomically restore the session it replaced.
    fn abandon_login<'a>(
        &'a self,
        id: String,
        prior: Option<(String, BrowserSessionState)>,
        now: u64,
    ) -> PortFuture<'a, ()>;
    /// Renew activity attributed to the credential just verified by the application.
    fn renew<'a>(
        &'a self,
        id: String,
        now: u64,
        initiator: CredentialId,
    ) -> PortFuture<'a, BrowserSessionState>;
}

pub fn invalidation_reason(error: AccessError) -> Option<RemovalReason> {
    match error {
        AccessError::CredentialRevoked => Some(RemovalReason::CredentialRevoked),
        AccessError::CredentialExpired => Some(RemovalReason::CredentialExpired),
        AccessError::InactiveMembership => Some(RemovalReason::InactiveMembership),
        AccessError::IdentityMismatch => Some(RemovalReason::IdentityMismatch),
        AccessError::InvalidCredential => Some(RemovalReason::InvalidCredential),
        AccessError::StaleRevision | AccessError::Unavailable | AccessError::Unsupported => None,
    }
}

/// Read through an injected store and enforce domain expiry independently of the adapter.
pub struct ReadBrowserSession<'a> {
    pub store: &'a dyn SessionStore,
}
impl ReadBrowserSession<'_> {
    pub async fn execute(
        &self,
        id: &str,
        now: u64,
    ) -> Result<Option<BrowserSessionState>, AccessError> {
        let Some(session) = self.store.get(id.to_owned()).await? else {
            return Ok(None);
        };
        let Some(reason) = session.expiration_reason(now) else {
            return Ok(Some(session));
        };
        self.store.remove(id.to_owned(), now, reason, None).await?;
        Ok(None)
    }
}

/// Verify an opaque cookie session against the current server-side session store.
/// The adapter binds the proof to the request origin and samples one injected time.
#[derive(Clone, Copy)]
pub struct BrowserSessionVerifier<'a> {
    pub store: &'a dyn SessionStore,
    pub expected_origin: &'a str,
    pub now: u64,
}
impl SessionVerifier for BrowserSessionVerifier<'_> {
    fn verify_session<'a>(
        &'a self,
        evidence: &'a SessionEvidence,
        _: &'a nessa_auth::domain::AudienceId,
    ) -> PortFuture<'a, VerifiedSessionCredential> {
        Box::pin(async move {
            let expected_origin = BrowserSessionOrigin::new(self.expected_origin.to_owned())
                .filter(|origin| is_trusted_origin_value(origin.as_str()))
                .ok_or(AccessError::InvalidCredential)?;
            let id = std::str::from_utf8(evidence.expose_bytes())
                .ok()
                .filter(|id| id.len() == 64 && id.as_bytes().iter().all(u8::is_ascii_hexdigit))
                .ok_or(AccessError::InvalidCredential)?;
            let session = ReadBrowserSession { store: self.store }
                .execute(id, self.now)
                .await?
                .filter(|session| session.origin() == expected_origin.as_str())
                .ok_or(AccessError::InvalidCredential)?;
            Ok(VerifiedSessionCredential {
                credential_id: session.credential_id().clone(),
            })
        })
    }
}

/// Establish a bounded browser session using the same identity verification as native clients.
pub struct SignIn<'a> {
    pub authentication: nessa_auth::application::session::AuthenticateSession<'a>,
    pub store: &'a dyn SessionStore,
    pub audience: &'a nessa_auth::domain::AudienceId,
}
impl SignIn<'_> {
    pub async fn execute(
        &self,
        token: String,
        origin: String,
        id: String,
        prior: Option<&str>,
    ) -> Result<(u64, Option<(String, BrowserSessionState)>), AccessError> {
        let origin = BrowserSessionOrigin::new(origin)
            .filter(|origin| is_trusted_origin_value(origin.as_str()))
            .ok_or(AccessError::InvalidCredential)?;
        let evidence = CredentialEvidence::new(token.into_bytes())?;
        let identity = self
            .authentication
            .execute(&evidence, self.audience)
            .await?;
        let now = self.authentication.clock.unix_seconds();
        let session =
            BrowserSessionState::new(identity.context().credential_id().clone(), origin, now)
                .ok_or(AccessError::InvalidCredential)?;
        let idle_expires_at = session.idle_expires_at();
        let replaced = self
            .store
            .insert(id, session, prior.map(str::to_owned), now)
            .await?;
        Ok((idle_expires_at - now, replaced))
    }
}
