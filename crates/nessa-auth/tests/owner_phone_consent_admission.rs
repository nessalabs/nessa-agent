//! Owner-phone consent must delegate every canonical action from one current Auth snapshot.

use nessa_auth::{
    adapters::cedar::CedarPolicyEvaluator,
    application::{
        ports::{
            AccessError, AccessReader, AccessSnapshot, Clock, CredentialEvidence,
            CredentialVerifier, PortFuture, VerifiedCredential,
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
struct TestStore(Arc<Mutex<AccessSnapshot>>, Arc<AtomicU64>);

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
            self.1.fetch_add(1, Ordering::SeqCst);
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
            store: TestStore(Arc::new(Mutex::new(snapshot)), Arc::new(AtomicU64::new(0))),
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

use nessa_auth::application::pairing::AuthorizePairing;
use nessa_auth::domain::pairing::{ConsentClass, ConsentIntent, ConsentIntentId};
fn owner_phone_intent() -> ConsentIntent {
    ConsentIntent::new(
        ConsentIntentId::new([7; 16]),
        1,
        audience_id(),
        principal_id(),
        membership_id(),
        resource("organization-1", "server-1"),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap()
}
fn delegate(
    fixture: &Fixture,
    session: &AuthenticatedSession,
    intent: &ConsentIntent,
) -> Result<(), AccessError> {
    ready(
        AuthorizePairing {
            access: &fixture.store,
            policy: &fixture.policy,
            clock: &fixture.clock,
        }
        .execute(session, intent, &resource("organization-1", "server-1")),
    )
    .map(|_| ())
}
#[test]
fn every_owner_phone_capability_requires_current_delegation() {
    let actions = [
        "credential.manage",
        "conversation.read",
        "conversation.write",
        "server.read",
    ];
    for missing in [
        None,
        Some("credential.manage"),
        Some("conversation.read"),
        Some("conversation.write"),
        Some("server.read"),
    ] {
        let fixture = Fixture::new(
            MembershipRole::Admin,
            actions
                .iter()
                .filter(|a| Some(**a) != missing)
                .map(|a| grant(a, "server-1"))
                .collect(),
        );
        let session = fixture.authenticate(b"correct secret", "server-1").unwrap();
        let before = fixture.store.1.load(Ordering::SeqCst);
        let result = delegate(&fixture, &session, &owner_phone_intent());
        assert_eq!(
            fixture.store.1.load(Ordering::SeqCst) - before,
            1,
            "one coherent admission snapshot"
        );
        if missing.is_some() {
            assert!(
                matches!(result, Err(AccessError::Denied)),
                "omitted {missing:?}: {result:?}"
            );
        } else {
            assert!(result.is_ok());
        }
    }
}
#[test]
fn current_revision_deadline_membership_and_original_identity_still_fence_consent() {
    let fixture = Fixture::new(
        MembershipRole::Admin,
        [
            "credential.manage",
            "conversation.read",
            "conversation.write",
            "server.read",
        ]
        .iter()
        .map(|a| grant(a, "server-1"))
        .collect(),
    );
    let session = fixture.authenticate(b"correct secret", "server-1").unwrap();
    let mut older = fixture.store.snapshot();
    older.revision = 0;
    fixture.store.replace(older);
    assert!(matches!(
        delegate(&fixture, &session, &owner_phone_intent()),
        Err(AccessError::StaleRevision)
    ));
    let mut current = fixture.store.snapshot();
    current.revision = 2;
    fixture.store.replace(current);
    assert!(delegate(&fixture, &session, &owner_phone_intent()).is_ok());
    let wrong = ConsentIntent::new(
        ConsentIntentId::new([8; 16]),
        1,
        audience_id(),
        PrincipalId::new("different-owner").unwrap(),
        membership_id(),
        resource("organization-1", "server-1"),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    assert!(matches!(
        delegate(&fixture, &session, &wrong),
        Err(AccessError::IdentityMismatch)
    ));
    let active = fixture.store.snapshot();
    let mut disabled = active.clone();
    disabled.membership = Membership::new(
        membership_id(),
        principal_id(),
        organization_id(),
        MembershipRole::Admin,
        MembershipStatus::Disabled,
    );
    fixture.store.replace(disabled);
    assert!(matches!(
        delegate(&fixture, &session, &owner_phone_intent()),
        Err(AccessError::InactiveMembership)
    ));
    fixture.store.replace(active);
    fixture.clock.set(PROOF_EXPIRES_AT);
    assert!(matches!(
        delegate(&fixture, &session, &owner_phone_intent()),
        Err(AccessError::CredentialExpired)
    ));
}
/// Every class, the owner phone's whole capability set included, can be
/// redeemed by code; such a record is not a signed one.
#[test]
fn owner_phone_capabilities_can_be_redeemed_by_code() {
    use nessa_auth::domain::pairing::{InvitationId, PairingPolicy, PairingRecord};
    let phone = PairingRecord::new(
        InvitationId::new([1; 16]),
        owner_phone_intent(),
        0,
        PairingPolicy::initial(),
    )
    .unwrap();
    assert!(!phone.signed_original());
    assert_eq!(phone.intent().grants().count(), 3);
    for class in [
        ConsentClass::DeviceRead,
        ConsentClass::PeerRead,
        ConsentClass::PeerEnvironment,
    ] {
        let intent = ConsentIntent::new(
            ConsentIntentId::new([7; 16]),
            1,
            audience_id(),
            principal_id(),
            membership_id(),
            resource("organization-1", "server-1"),
            class,
        )
        .unwrap();
        assert_eq!(intent.grants().count(), 1);
        assert!(PairingRecord::new(
            InvitationId::new([1; 16]),
            intent,
            0,
            PairingPolicy::initial()
        )
        .is_ok());
    }
}
