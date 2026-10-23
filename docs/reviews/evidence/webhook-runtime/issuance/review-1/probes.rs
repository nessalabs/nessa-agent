
use super::tests::ready;
use crate::{
    adapters::cedar::CedarPolicyEvaluator,
    application::{
        authorization::AuthorizeAction,
        ports::Clock,
        session::AuthenticateSession,
    },
    domain::{Action, Grant},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ReviewTime(u64);
impl Clock for ReviewTime {
    fn unix_milliseconds(&self) -> u64 { self.0 }
}

#[test]
fn independent_bearer_authorization_keeps_read_no_write_and_current_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth/credentials.v1.json");
    let store = open_store(&path).unwrap();
    store.bootstrap(bootstrap()).unwrap();
    let IssueCredentialOutcome::Issued { evidence, .. } = store.issue_sync(request()).unwrap() else { panic!("fresh issue") };
    let clock = ReviewTime(130_000);
    let session = ready(AuthenticateSession { verifier: &store, access: &store, clock: &clock }.execute(&evidence, &AudienceId::new("gateway-1").unwrap())).unwrap();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let gateway = Resource::new(OrganizationId::new("org-1").unwrap(), ResourceId::new("gateway-1").unwrap());
    let authorize = AuthorizeAction { access: &store, clock: &clock, policy: &policy };
    for (action, expected) in [("conversation.read", Decision::Allow), ("conversation.write", Decision::Deny), ("credential.manage", Decision::Deny), ("server.read", Decision::Deny), ("environment.serve", Decision::Deny)] {
        assert_eq!(ready(authorize.execute(&session, &Action::new(action).unwrap(), &gateway)), Ok(expected), "{action}");
    }
    let expired = ReviewTime(190_000);
    assert_eq!(ready(AuthorizeAction { access: &store, clock: &expired, policy: &policy }.execute(&session, &Action::new("conversation.read").unwrap(), &gateway)), Err(AccessError::CredentialExpired));
    store.revoke_sync(RevokeCredentialRequest { request_id: "revoked-reader".into(), issuer_principal_id: "owner".into(), credential_id: "webhook-credential".into(), revoked_at: 140 }).unwrap();
    assert_eq!(ready(authorize.execute(&session, &Action::new("conversation.read").unwrap(), &gateway)), Err(AccessError::CredentialRevoked));
}

#[test]
fn independent_cold_replay_preserves_issue_evidence_but_returns_current_revoked_metadata() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth/credentials.v1.json");
    let store = open_store(&path).unwrap();
    store.bootstrap(bootstrap()).unwrap();
    let IssueCredentialOutcome::Issued { metadata, transitions, .. } = store.issue_sync(request()).unwrap() else { panic!("fresh") };
    assert_eq!(metadata.revoked_at, None);
    let revoked = store.revoke_sync(RevokeCredentialRequest { request_id: "stop".into(), issuer_principal_id: "owner".into(), credential_id: metadata.id.clone(), revoked_at: 140 }).unwrap();
    drop(store);
    let store = open_store(&path).unwrap();
    let before = fs::read(&path).unwrap();
    let IssueCredentialOutcome::ExistingSecretUnavailable { metadata: current, transitions: original } = store.issue_sync(request()).unwrap() else { panic!("no secret replay") };
    assert_eq!(current.id, metadata.id);
    assert_eq!(current.revoked_at, Some(140));
    assert_eq!(original, transitions);
    assert_eq!(revoked.revocation.after.revoked_at, Some(140));
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut switched = request(); switched.purpose = CredentialIssuePurpose::Product;
    assert!(matches!(store.issue_sync(switched), Err(LocalStoreError::Conflict)));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn independent_malformed_webhook_requests_and_revoke_namespace_write_nothing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth/credentials.v1.json");
    let store = open_store(&path).unwrap();
    store.bootstrap(bootstrap()).unwrap();
    let before = fs::read(&path).unwrap();
    let mut invalids = Vec::new();
    let mut empty = request(); empty.grants.clear(); invalids.push(empty);
    let mut unknown = request(); unknown.grants[0].action = "unknown.action".into(); invalids.push(unknown);
    let mut issuer = request(); issuer.issuer_principal_id = "missing".into(); invalids.push(issuer);
    let mut disabled = request(); disabled.membership.state = MembershipStateDto::Disabled; invalids.push(disabled);
    let mut mismatch = request(); mismatch.membership.principal_id = "another".into(); invalids.push(mismatch);
    let mut expired = request(); expired.expires_at = Some(expired.issued_at); invalids.push(expired);
    for invalid in invalids { assert!(matches!(store.issue_sync(invalid), Err(LocalStoreError::Conflict))); assert_eq!(fs::read(&path).unwrap(), before); }
    store.issue_sync(request()).unwrap();
    let issued = fs::read(&path).unwrap();
    assert!(matches!(store.revoke_sync(RevokeCredentialRequest { request_id: request().request_id, issuer_principal_id: "owner".into(), credential_id: "webhook-credential".into(), revoked_at: 140 }), Err(LocalStoreError::Conflict)));
    assert_eq!(fs::read(&path).unwrap(), issued);
    store.revoke_sync(RevokeCredentialRequest { request_id: "stop".into(), issuer_principal_id: "owner".into(), credential_id: "webhook-credential".into(), revoked_at: 140 }).unwrap();
    drop(store);
    let store = open_store(&path).unwrap();
    let stopped = fs::read(&path).unwrap();
    let mut conflict = request(); conflict.request_id = "stop".into(); conflict.credential_id = "unused".into();
    assert!(matches!(store.issue_sync(conflict), Err(LocalStoreError::Conflict)));
    assert_eq!(fs::read(&path).unwrap(), stopped);
}

struct ReviewPolicy { cedar: CedarPolicyEvaluator, holds: AtomicUsize, issues: AtomicUsize }
impl GrantPolicyEvaluator for ReviewPolicy {
    fn may_hold(&self, kind: PrincipalKind, grant: &Grant, path: IssuePath) -> Result<Decision, AccessError> {
        if path == IssuePath::WebhookIssue {
            self.holds.fetch_add(1, Ordering::SeqCst);
            return Ok(if grant.action().as_str() == "conversation.write" { Decision::Allow } else { Decision::Deny });
        }
        self.cedar.may_hold(kind, grant, path)
    }
    fn may_issue(&self, kind: PrincipalKind, role: MembershipRole, resource: &Resource, path: IssuePath) -> Result<Decision, AccessError> {
        if path == IssuePath::WebhookIssue { self.issues.fetch_add(1, Ordering::SeqCst); }
        self.cedar.may_issue(kind, role, resource, path)
    }
}
#[test]
fn independent_injected_policy_owns_webhook_grants_without_a_second_rust_rule() {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let policy = Arc::new(ReviewPolicy { cedar: CedarPolicyEvaluator::new().unwrap(), holds: AtomicUsize::new(0), issues: AtomicUsize::new(0) });
    let store = LocalCredentialStore::open(&private, "auth.json", policy.clone()).unwrap();
    store.bootstrap(bootstrap()).unwrap();
    let before = fs::read(private.join("auth.json")).unwrap();
    assert!(matches!(store.issue_sync(request()), Err(LocalStoreError::Conflict)));
    assert_eq!(fs::read(private.join("auth.json")).unwrap(), before);
    let mut command = request(); command.grants[0].action = "conversation.write".into();
    let IssueCredentialOutcome::Issued { metadata, .. } = store.issue_sync(command).unwrap() else { panic!("test-only alternate policy is the owner") };
    assert_eq!(metadata.grants[0].action, "conversation.write");
    assert_eq!(policy.holds.load(Ordering::SeqCst), 2);
    assert_eq!(policy.issues.load(Ordering::SeqCst), 2);
}
