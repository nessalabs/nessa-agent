//! Authentication assembles identity only. Callers must authorize each operation
//! against fresh state and own invalidation/expiry handling for open connections.
use super::ports::{
    AccessError, AccessReader, Clock, CredentialEvidence, CredentialVerifier, SessionEvidence,
    SessionVerifier,
};
use crate::domain::{AudienceId, AuthContext};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Verified session identity plus the lifetime and state revision checked at entry.
/// This result grants no action and does not subscribe to future invalidations.
pub struct AuthenticatedSession {
    /// Stable verified identity selectors; no cached role or permission.
    context: AuthContext,
    /// Exclusive session expiration in Unix seconds.
    expires_at: Option<u64>,
    /// Committed revision checked during authentication, not a freshness guarantee.
    auth_revision: u64,
}

impl AuthenticatedSession {
    /// Stable identity verified during authentication, without cached permissions.
    pub fn context(&self) -> &AuthContext {
        &self.context
    }

    /// Exclusive session deadline in Unix seconds; callers cannot extend it.
    pub fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }

    /// Committed revision at authentication, used to reject older snapshots.
    pub fn auth_revision(&self) -> u64 {
        self.auth_revision
    }
}

/// Validate a session against one current committed snapshot, without granting
/// permission to perform an action. Shared by gateway liveness and authorization.
pub struct ReadCurrentSession<'a> {
    /// Current credential and membership state.
    pub access: &'a dyn AccessReader,
    /// Absolute expiry clock.
    pub clock: &'a dyn Clock,
}

impl ReadCurrentSession<'_> {
    /// Reject revoked, expired, inactive, mismatched, or stale identity state.
    pub async fn execute(
        &self,
        session: &AuthenticatedSession,
    ) -> Result<super::ports::AccessSnapshot, AccessError> {
        let context = session.context();
        let snapshot = self.access.read(context.credential_id()).await?;
        if snapshot.revision < session.auth_revision() {
            return Err(AccessError::StaleRevision);
        }
        let credential = &snapshot.credential;
        let membership = &snapshot.membership;
        let now = self.clock.unix_seconds();
        if credential.revoked_at().is_some() {
            return Err(AccessError::CredentialRevoked);
        }
        if session.expires_at().is_some_and(|expiry| now >= expiry)
            || credential.expires_at().is_some_and(|expiry| now >= expiry)
        {
            return Err(AccessError::CredentialExpired);
        }
        if !credential.is_valid_at(now) {
            return Err(AccessError::InvalidCredential);
        }
        if credential.id() != context.credential_id()
            || credential.principal_id() != context.principal_id()
            || credential.organization_id() != context.organization_id()
            || credential.audience_id() != context.audience_id()
            || membership.id() != context.membership_id()
            || membership.principal_id() != context.principal_id()
            || membership.organization_id() != context.organization_id()
        {
            return Err(AccessError::IdentityMismatch);
        }
        if !membership.is_active() {
            return Err(AccessError::InactiveMembership);
        }
        Ok(snapshot)
    }
}

/// Reconstruct current identity only after a trusted adapter verifies an opaque
/// browser or host session. Raw credential IDs cannot invoke this boundary.
pub struct ResumeSession<'a> {
    /// Verifies opaque session evidence and returns its credential binding.
    pub verifier: &'a dyn SessionVerifier,
    /// Reads current credential and membership state.
    pub access: &'a dyn AccessReader,
    /// Absolute expiry clock.
    pub clock: &'a dyn Clock,
}
impl ResumeSession<'_> {
    /// Verify `evidence`, then reconstruct identity from current registry state.
    pub async fn execute(
        &self,
        evidence: &SessionEvidence,
        audience: &AudienceId,
    ) -> Result<(AuthenticatedSession, super::ports::AccessSnapshot), AccessError> {
        let verified = self.verifier.verify_session(evidence, audience).await?;
        let credential_id = verified.credential_id;
        let snapshot = self.access.read(&credential_id).await?;
        let credential = &snapshot.credential;
        let membership = &snapshot.membership;
        let now = self.clock.unix_seconds();
        if credential.revoked_at().is_some() {
            return Err(AccessError::CredentialRevoked);
        }
        if credential.expires_at().is_some_and(|expiry| now >= expiry) {
            return Err(AccessError::CredentialExpired);
        }
        if !credential.is_valid_at(now) {
            return Err(AccessError::InvalidCredential);
        }
        if credential.id() != &credential_id
            || credential.audience_id() != audience
            || credential.principal_id() != membership.principal_id()
            || credential.organization_id() != membership.organization_id()
        {
            return Err(AccessError::IdentityMismatch);
        }
        if !membership.is_active() {
            return Err(AccessError::InactiveMembership);
        }
        let session = AuthenticatedSession {
            context: AuthContext::new(
                credential.principal_id().clone(),
                credential.organization_id().clone(),
                membership.id().clone(),
                credential.id().clone(),
                credential.audience_id().clone(),
            ),
            expires_at: credential.expires_at(),
            auth_revision: snapshot.revision,
        };
        Ok((session, snapshot))
    }
}

/// Application use case assembled from trusted adapters by the composition root.
/// Borrows dependencies; owns no connections, global state, or background tasks.
pub struct AuthenticateSession<'a> {
    /// Trusted proof verifier and credential-binding resolver.
    pub verifier: &'a dyn CredentialVerifier,
    /// Reader of coherent current credential/membership state.
    pub access: &'a dyn AccessReader,
    /// Absolute-time source sampled to check credential and proof expiry.
    pub clock: &'a dyn Clock,
}
impl AuthenticateSession<'_> {
    /// Authenticate opaque `evidence` for the configured deployment `audience`.
    ///
    /// Verifies proof, reads one access snapshot, checks binding/membership linkage,
    /// and samples the clock after those reads. The result expires at the earlier
    /// of proof and credential expiry. Verifier/store failures propagate; invalid
    /// lifetime, mismatched identity, and disabled membership return typed errors.
    ///
    /// Callers must enforce the returned deadline and reauthorize operations with
    /// current state. A revocation after the snapshot read is handled by gateway
    /// invalidation/admission rules, not by this one-time authentication call.
    pub async fn execute(
        &self,
        evidence: &CredentialEvidence,
        audience: &AudienceId,
    ) -> Result<AuthenticatedSession, AccessError> {
        let proof = self.verifier.verify(evidence, audience).await?;
        let id = proof.credential_id;
        let snapshot = self.access.read(&id).await?;
        let credential = &snapshot.credential;
        let membership = &snapshot.membership;
        if credential.id() != &id || credential.audience_id() != audience {
            return Err(AccessError::IdentityMismatch);
        }
        let now = self.clock.unix_seconds();
        if proof.expires_at.is_some_and(|expiry| expiry <= now) || !credential.is_valid_at(now) {
            return Err(AccessError::InvalidCredential);
        }
        if credential.principal_id() != membership.principal_id()
            || credential.organization_id() != membership.organization_id()
        {
            return Err(AccessError::IdentityMismatch);
        }
        if !membership.is_active() {
            return Err(AccessError::InactiveMembership);
        }
        Ok(AuthenticatedSession {
            context: AuthContext::new(
                credential.principal_id().clone(),
                credential.organization_id().clone(),
                membership.id().clone(),
                credential.id().clone(),
                credential.audience_id().clone(),
            ),
            expires_at: proof
                .expires_at
                .into_iter()
                .chain(credential.expires_at())
                .min(),
            auth_revision: snapshot.revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::ports::{
            AccessSnapshot, PortFuture, VerifiedCredential, VerifiedSessionCredential,
        },
        domain::*,
    };
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    struct Adapter {
        snapshot: AccessSnapshot,
        now: u64,
    }
    impl CredentialVerifier for Adapter {
        fn verify<'a>(
            &'a self,
            evidence: &'a CredentialEvidence,
            _: &'a AudienceId,
        ) -> PortFuture<'a, VerifiedCredential> {
            Box::pin(async move {
                if evidence.expose_bytes() != b"test" {
                    return Err(AccessError::InvalidCredential);
                }
                Ok(VerifiedCredential {
                    credential_id: CredentialId::new("credential").unwrap(),
                    expires_at: Some(18),
                })
            })
        }
    }
    impl SessionVerifier for Adapter {
        fn verify_session<'a>(
            &'a self,
            evidence: &'a SessionEvidence,
            _: &'a AudienceId,
        ) -> PortFuture<'a, VerifiedSessionCredential> {
            Box::pin(async move {
                if evidence.expose_bytes() != b"browser-session" {
                    return Err(AccessError::InvalidCredential);
                }
                Ok(VerifiedSessionCredential {
                    credential_id: CredentialId::new("credential").unwrap(),
                })
            })
        }
    }
    impl AccessReader for Adapter {
        fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
            Box::pin(async { Ok(self.snapshot.clone()) })
        }
    }
    impl Clock for Adapter {
        fn unix_milliseconds(&self) -> u64 {
            self.now * 1000
        }
    }
    fn fixture(org: &str, status: MembershipStatus) -> Adapter {
        Adapter {
            snapshot: AccessSnapshot {
                credential: Credential::new(
                    CredentialId::new("credential").unwrap(),
                    PrincipalId::new("person").unwrap(),
                    OrganizationId::new("org").unwrap(),
                    AudienceId::new("local").unwrap(),
                    10,
                    20,
                    vec![],
                )
                .unwrap(),
                membership: Membership::new(
                    MembershipId::new("member").unwrap(),
                    PrincipalId::new("person").unwrap(),
                    OrganizationId::new(org).unwrap(),
                    MembershipRole::Member,
                    status,
                ),
                revision: 1,
            },
            now: 10,
        }
    }
    // These contract doubles complete immediately. No async runtime dependency.
    fn ready<T>(future: impl Future<Output = T>) -> T {
        let waker = Waker::noop();
        match std::pin::pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(waker))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test adapter unexpectedly yielded"),
        }
    }
    fn authenticate(
        adapter: &Adapter,
        audience: &str,
        token: &[u8],
    ) -> Result<AuthenticatedSession, AccessError> {
        ready(
            AuthenticateSession {
                verifier: adapter,
                access: adapter,
                clock: adapter,
            }
            .execute(
                &CredentialEvidence::new(token.to_vec()).unwrap(),
                &AudienceId::new(audience).unwrap(),
            ),
        )
    }
    #[test]
    fn opaque_session_resolves_current_identity_revision_and_audience() {
        let adapter = fixture("org", MembershipStatus::Active);
        let audience = AudienceId::new("local").unwrap();
        let id = CredentialId::new("credential").unwrap();
        let reader = ResumeSession {
            verifier: &adapter,
            access: &adapter,
            clock: &adapter,
        };
        let evidence = SessionEvidence::new(b"browser-session".to_vec()).unwrap();
        let (session, snapshot) = ready(reader.execute(&evidence, &audience)).unwrap();
        assert_eq!(session.context().credential_id(), &id);
        assert_eq!(session.context().membership_id().as_str(), "member");
        assert_eq!(session.auth_revision(), snapshot.revision);
        assert_eq!(session.expires_at(), Some(20));
        assert!(matches!(
            ready(reader.execute(&evidence, &AudienceId::new("other").unwrap())),
            Err(AccessError::IdentityMismatch)
        ));
        let forged = SessionEvidence::new(b"credential".to_vec()).unwrap();
        assert!(matches!(
            ready(reader.execute(&forged, &audience)),
            Err(AccessError::InvalidCredential)
        ));
        let mut expired = adapter;
        expired.now = 20;
        assert!(matches!(
            ready(
                ResumeSession {
                    verifier: &expired,
                    access: &expired,
                    clock: &expired,
                }
                .execute(&evidence, &audience)
            ),
            Err(AccessError::CredentialExpired)
        ));
    }

    #[test]
    fn opaque_session_resolution_rejects_current_revocation_membership_and_linkage() {
        let audience = AudienceId::new("local").unwrap();
        let evidence = SessionEvidence::new(b"browser-session".to_vec()).unwrap();
        let mut revoked = fixture("org", MembershipStatus::Active);
        revoked.snapshot.credential.revoke(10).unwrap();
        assert!(matches!(
            ready(
                ResumeSession {
                    verifier: &revoked,
                    access: &revoked,
                    clock: &revoked,
                }
                .execute(&evidence, &audience)
            ),
            Err(AccessError::CredentialRevoked)
        ));

        let inactive = fixture("org", MembershipStatus::Disabled);
        assert!(matches!(
            ready(
                ResumeSession {
                    verifier: &inactive,
                    access: &inactive,
                    clock: &inactive,
                }
                .execute(&evidence, &audience)
            ),
            Err(AccessError::InactiveMembership)
        ));

        let mut current = fixture("org", MembershipStatus::Active);
        current.snapshot.credential = Credential::new(
            CredentialId::new("other-credential").unwrap(),
            PrincipalId::new("person").unwrap(),
            OrganizationId::new("org").unwrap(),
            AudienceId::new("local").unwrap(),
            10,
            20,
            vec![],
        )
        .unwrap();
        assert!(matches!(
            ready(
                ResumeSession {
                    verifier: &current,
                    access: &current,
                    clock: &current,
                }
                .execute(&evidence, &audience)
            ),
            Err(AccessError::IdentityMismatch)
        ));
    }

    #[test]
    fn independent_injected_adapters_do_not_share_identity_state() {
        let good = fixture("org", MembershipStatus::Active);
        let other = fixture("other", MembershipStatus::Active);
        let authenticated = authenticate(&good, "local", b"test").unwrap();
        assert_eq!(authenticated.expires_at, Some(18));
        assert_eq!(authenticated.auth_revision, 1);
        assert_eq!(
            authenticate(&good, "local", b"test")
                .unwrap()
                .context
                .organization_id()
                .as_str(),
            "org"
        );
        assert_eq!(
            authenticate(&other, "local", b"test"),
            Err(AccessError::IdentityMismatch)
        );
        assert!(authenticate(&good, "local", b"test").is_ok());
    }
    #[test]
    fn rejects_invalid_secret_audience_membership_and_time() {
        let mut adapter = fixture("org", MembershipStatus::Active);
        assert_eq!(
            authenticate(&adapter, "local", b"wrong"),
            Err(AccessError::InvalidCredential)
        );
        assert_eq!(
            authenticate(&adapter, "other", b"test"),
            Err(AccessError::IdentityMismatch)
        );
        for now in [9, 18, 20, 21] {
            adapter.now = now;
            assert_eq!(
                authenticate(&adapter, "local", b"test"),
                Err(AccessError::InvalidCredential)
            );
        }
        let disabled = fixture("org", MembershipStatus::Disabled);
        assert_eq!(
            authenticate(&disabled, "local", b"test"),
            Err(AccessError::InactiveMembership)
        );
        adapter.now = 15;
        adapter.snapshot.credential.revoke(15).unwrap();
        assert_eq!(
            authenticate(&adapter, "local", b"test"),
            Err(AccessError::InvalidCredential)
        );
    }
}
