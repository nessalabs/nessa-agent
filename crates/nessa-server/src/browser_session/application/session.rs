use crate::browser_session::domain::value_objects::{Lifetime, RemovalReason};
use nessa_auth::application::ports::{
    AccessError, CredentialEvidence, PortFuture, SessionEvidence, SessionVerifier,
    VerifiedSessionCredential,
};
use nessa_auth::domain::CredentialId;

const RENEWAL_INTERVAL_SECONDS: u64 = 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserSession {
    credential_id: CredentialId,
    lifetime: Lifetime,
    origin: String,
}

/// Bounded session persistence selected by server composition.
pub trait SessionStore: Send + Sync {
    /// Insert one session and return the exact active `prior` atomically replaced.
    fn insert<'a>(
        &'a self,
        id: String,
        session: BrowserSession,
        prior: Option<String>,
        now: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSession)>>;
    fn get<'a>(&'a self, id: String) -> PortFuture<'a, Option<BrowserSession>>;
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
        prior: Option<(String, BrowserSession)>,
        now: u64,
    ) -> PortFuture<'a, ()>;
    /// Renew activity attributed to the credential just verified by the application.
    fn renew<'a>(
        &'a self,
        id: String,
        now: u64,
        initiator: CredentialId,
    ) -> PortFuture<'a, BrowserSession>;
}

impl BrowserSession {
    pub fn new(credential_id: CredentialId, origin: String, now: u64) -> Option<Self> {
        Some(Self {
            lifetime: Lifetime::new(now)?,
            credential_id,
            origin,
        })
    }
    pub fn restore(
        credential_id: CredentialId,
        origin: String,
        created_at: u64,
        renewed_at: u64,
        idle_expires_at: u64,
    ) -> Option<Self> {
        Some(Self {
            lifetime: Lifetime::restore(created_at, renewed_at, idle_expires_at)?,
            credential_id,
            origin,
        })
    }
    pub fn renewed_for_check(&self, now: u64) -> Option<Self> {
        if !self.is_active_at(now) {
            return None;
        }
        if now.saturating_sub(self.renewed_at()) < RENEWAL_INTERVAL_SECONDS {
            return Some(self.clone());
        }
        let lifetime = self.lifetime.renew(now)?;
        Some(Self {
            credential_id: self.credential_id.clone(),
            origin: self.origin.clone(),
            lifetime,
        })
    }
    pub fn is_active_at(&self, now: u64) -> bool {
        self.lifetime.is_active_at(now)
    }
    pub fn expiration_reason(&self, now: u64) -> Option<RemovalReason> {
        if self.is_active_at(now) {
            None
        } else {
            Some(RemovalReason::IdleExpired)
        }
    }
    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn created_at(&self) -> u64 {
        self.lifetime.created_at()
    }
    pub fn renewed_at(&self) -> u64 {
        self.lifetime.renewed_at()
    }
    pub fn idle_expires_at(&self) -> u64 {
        self.lifetime.idle_expires_at()
    }
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
    pub async fn execute(&self, id: &str, now: u64) -> Result<Option<BrowserSession>, AccessError> {
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
            let id = std::str::from_utf8(evidence.expose_bytes())
                .ok()
                .filter(|id| id.len() == 64 && id.as_bytes().iter().all(u8::is_ascii_hexdigit))
                .ok_or(AccessError::InvalidCredential)?;
            let session = ReadBrowserSession { store: self.store }
                .execute(id, self.now)
                .await?
                .filter(|session| session.origin() == self.expected_origin)
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
    ) -> Result<(u64, Option<(String, BrowserSession)>), AccessError> {
        let evidence = CredentialEvidence::new(token.into_bytes())?;
        let identity = self
            .authentication
            .execute(&evidence, self.audience)
            .await?;
        let now = self.authentication.clock.unix_seconds();
        let session = BrowserSession::new(identity.context().credential_id().clone(), origin, now)
            .ok_or(AccessError::InvalidCredential)?;
        let idle_expires_at = session.idle_expires_at();
        let replaced = self
            .store
            .insert(id, session, prior.map(str::to_owned), now)
            .await?;
        Ok((idle_expires_at - now, replaced))
    }
}
