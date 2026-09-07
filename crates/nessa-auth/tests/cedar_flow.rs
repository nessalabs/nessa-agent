//! Exercises the public authentication and authorization boundary with the real
//! Cedar policy adapter. Test doubles provide only proof, storage, and time.

use nessa_auth::{
    adapters::cedar::CedarPolicyEvaluator,
    application::{
        authorization::AuthorizeAction,
        ports::{
            AccessError, AccessReader, AccessSnapshot, Clock, CredentialEvidence,
            CredentialVerifier, Decision, PortFuture, VerifiedCredential,
        },
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{
        Action, AudienceId, Credential, CredentialId, Grant, Membership, MembershipId,
        MembershipRole, MembershipStatus, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
};

const ISSUED_AT: u64 = 100;
const EXPIRES_AT: u64 = 200;
const PROOF_EXPIRES_AT: u64 = 190;

#[derive(Clone)]
struct TestVerifier {
    credential_id: CredentialId,
    audience_id: AudienceId,
}

impl CredentialVerifier for TestVerifier {
    fn verify<'a>(
        &'a self,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            if evidence.expose_bytes() != b"correct secret" || audience != &self.audience_id {
                return Err(AccessError::InvalidCredential);
            }
            Ok(VerifiedCredential {
                credential_id: self.credential_id.clone(),
                expires_at: Some(PROOF_EXPIRES_AT),
            })
        })
    }
}

#[derive(Clone)]
struct TestStore(Arc<Mutex<AccessSnapshot>>);

impl TestStore {
    fn replace(&self, snapshot: AccessSnapshot) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = snapshot;
    }

    fn snapshot(&self) -> AccessSnapshot {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl AccessReader for TestStore {
    fn read<'a>(&'a self, credential_id: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            let snapshot = self.snapshot();
            if snapshot.credential.id() != credential_id {
                return Err(AccessError::InvalidCredential);
            }
            Ok(snapshot)
        })
    }
}

#[derive(Clone)]
struct TestClock(Arc<AtomicU64>);

impl TestClock {
    fn set(&self, now: u64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst) * 1000
    }
}

struct Fixture {
    verifier: TestVerifier,
    store: TestStore,
    clock: TestClock,
    policy: CedarPolicyEvaluator,
}

impl Fixture {
    fn new(role: MembershipRole, grants: Vec<Grant>) -> Self {
        let snapshot = snapshot(role, MembershipStatus::Active, grants);
        Self {
            verifier: TestVerifier {
                credential_id: credential_id(),
                audience_id: audience_id(),
            },
            store: TestStore(Arc::new(Mutex::new(snapshot))),
            clock: TestClock(Arc::new(AtomicU64::new(ISSUED_AT))),
            policy: CedarPolicyEvaluator::new().expect("embedded Cedar policy is valid"),
        }
    }

    fn authenticate(
        &self,
        secret: &[u8],
        audience: &str,
    ) -> Result<AuthenticatedSession, AccessError> {
        ready(
            AuthenticateSession {
                verifier: &self.verifier,
                access: &self.store,
                clock: &self.clock,
            }
            .execute(
                &CredentialEvidence::new(secret.to_vec()).unwrap(),
                &AudienceId::new(audience).unwrap(),
            ),
        )
    }

    fn authorize(
        &self,
        session: &AuthenticatedSession,
        action: &str,
        resource: Resource,
    ) -> Result<Decision, AccessError> {
        ready(
            AuthorizeAction {
                access: &self.store,
                clock: &self.clock,
                policy: &self.policy,
            }
            .execute(session, &Action::new(action).unwrap(), &resource),
        )
    }
}

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

fn principal_id() -> PrincipalId {
    PrincipalId::new("principal-1").unwrap()
}

fn organization_id() -> OrganizationId {
    OrganizationId::new("organization-1").unwrap()
}

fn membership_id() -> MembershipId {
    MembershipId::new("membership-1").unwrap()
}

fn credential_id() -> CredentialId {
    CredentialId::new("credential-1").unwrap()
}

fn audience_id() -> AudienceId {
    AudienceId::new("server-1").unwrap()
}

fn resource(organization: &str, id: &str) -> Resource {
    Resource::new(
        OrganizationId::new(organization).unwrap(),
        ResourceId::new(id).unwrap(),
    )
}

fn grant(action: &str, id: &str) -> Grant {
    Grant::new(Action::new(action).unwrap(), resource("organization-1", id))
}

fn snapshot(role: MembershipRole, status: MembershipStatus, grants: Vec<Grant>) -> AccessSnapshot {
    AccessSnapshot {
        credential: Credential::new(
            credential_id(),
            principal_id(),
            organization_id(),
            audience_id(),
            ISSUED_AT,
            EXPIRES_AT,
            grants,
        )
        .unwrap(),
        membership: Membership::new(
            membership_id(),
            principal_id(),
            organization_id(),
            role,
            status,
        ),
        revision: 1,
    }
}

#[test]
fn cedar_enforces_exact_grants_and_current_membership_role() {
    let fixture = Fixture::new(
        MembershipRole::Member,
        vec![
            grant("server.read", "server-a"),
            grant("credential.manage", "credential-a"),
        ],
    );
    let session = fixture.authenticate(b"correct secret", "server-1").unwrap();
    assert_eq!(session.expires_at(), Some(PROOF_EXPIRES_AT));

    assert_eq!(
        fixture.authorize(
            &session,
            "server.read",
            resource("organization-1", "server-a")
        ),
        Ok(Decision::Allow)
    );
    assert_eq!(
        fixture.authorize(
            &session,
            "credential.manage",
            resource("organization-1", "credential-a")
        ),
        Ok(Decision::Deny)
    );

    fixture.store.replace(snapshot(
        MembershipRole::Admin,
        MembershipStatus::Active,
        vec![grant("credential.manage", "credential-a")],
    ));
    assert_eq!(
        fixture.authorize(
            &session,
            "credential.manage",
            resource("organization-1", "credential-a")
        ),
        Ok(Decision::Allow)
    );
    assert_eq!(
        fixture.authorize(
            &session,
            "credential.manage",
            resource("organization-1", "credential-b")
        ),
        Ok(Decision::Deny)
    );
    assert_eq!(
        fixture.authorize(
            &session,
            "unknown.action",
            resource("organization-1", "credential-a")
        ),
        Ok(Decision::Deny)
    );
    assert_eq!(
        fixture.authorize(
            &session,
            "credential.manage",
            resource("organization-2", "credential-a")
        ),
        Ok(Decision::Deny)
    );
}

#[test]
fn authorization_reloads_state_after_login() {
    let fixture = Fixture::new(
        MembershipRole::Admin,
        vec![grant("credential.manage", "credential-a")],
    );
    let session = fixture.authenticate(b"correct secret", "server-1").unwrap();
    let target = || resource("organization-1", "credential-a");

    let mut stale = snapshot(
        MembershipRole::Admin,
        MembershipStatus::Active,
        vec![grant("credential.manage", "credential-a")],
    );
    stale.revision = 0;
    fixture.store.replace(stale);
    assert_eq!(
        fixture.authorize(&session, "credential.manage", target()),
        Err(AccessError::Unavailable)
    );

    fixture.store.replace(snapshot(
        MembershipRole::Member,
        MembershipStatus::Active,
        vec![grant("credential.manage", "credential-a")],
    ));
    assert_eq!(
        fixture.authorize(&session, "credential.manage", target()),
        Ok(Decision::Deny)
    );

    fixture.store.replace(snapshot(
        MembershipRole::Admin,
        MembershipStatus::Active,
        vec![],
    ));
    assert_eq!(
        fixture.authorize(&session, "credential.manage", target()),
        Ok(Decision::Deny)
    );

    fixture.store.replace(snapshot(
        MembershipRole::Admin,
        MembershipStatus::Disabled,
        vec![grant("credential.manage", "credential-a")],
    ));
    assert_eq!(
        fixture.authorize(&session, "credential.manage", target()),
        Err(AccessError::InactiveMembership)
    );

    let mut revoked = snapshot(
        MembershipRole::Admin,
        MembershipStatus::Active,
        vec![grant("credential.manage", "credential-a")],
    );
    revoked.credential.revoke(150).unwrap();
    fixture.store.replace(revoked);
    assert_eq!(
        fixture.authorize(&session, "credential.manage", target()),
        Err(AccessError::CredentialRevoked)
    );

    fixture.store.replace(snapshot(
        MembershipRole::Admin,
        MembershipStatus::Active,
        vec![grant("credential.manage", "credential-a")],
    ));
    fixture.clock.set(PROOF_EXPIRES_AT);
    assert_eq!(
        fixture.authorize(&session, "credential.manage", target()),
        Err(AccessError::CredentialExpired)
    );
}

#[test]
fn authentication_rejects_wrong_secret_and_audience() {
    let fixture = Fixture::new(MembershipRole::Member, vec![]);
    assert_eq!(
        fixture.authenticate(b"wrong secret", "server-1"),
        Err(AccessError::InvalidCredential)
    );
    assert_eq!(
        fixture.authenticate(b"correct secret", "other-server"),
        Err(AccessError::InvalidCredential)
    );
}
