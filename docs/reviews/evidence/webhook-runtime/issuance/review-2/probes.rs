
use crate::{adapters::cedar::CedarPolicyEvaluator, domain::{Grant, MembershipRole}};

struct FinalReviewPolicy {
    cedar: CedarPolicyEvaluator,
    recipient: bool,
    answer: Result<Decision, AccessError>,
}
impl GrantPolicyEvaluator for FinalReviewPolicy {
    fn may_hold(&self, kind: PrincipalKind, grant: &Grant, path: IssuePath) -> Result<Decision, AccessError> {
        if path == IssuePath::WebhookIssue && !self.recipient { return self.answer; }
        self.cedar.may_hold(kind,grant,path)
    }
    fn may_issue(&self, kind: PrincipalKind, role: MembershipRole, resource: &Resource, path: IssuePath) -> Result<Decision, AccessError> {
        if path == IssuePath::WebhookIssue && self.recipient { return self.answer; }
        self.cedar.may_issue(kind,role,resource,path)
    }
}

#[test]
fn final_review_policy_unavailability_and_deny_refuse_before_effects_at_both_stages() {
    for recipient in [false,true] {
        for answer in [Ok(Decision::Deny),Err(AccessError::Unavailable)] {
            let root=tempfile::tempdir().unwrap();
            let private=root.path().join("private");
            nessa_local_storage::create_directory(&private).unwrap();
            let policy=Arc::new(FinalReviewPolicy{cedar:CedarPolicyEvaluator::new().unwrap(),recipient,answer});
            let store=LocalCredentialStore::open(&private,"auth.json",policy).unwrap();
            store.bootstrap(bootstrap()).unwrap();
            let before=fs::read(private.join("auth.json")).unwrap();
            assert!(matches!(store.issue_sync(request()),Err(LocalStoreError::Conflict)),"recipient={recipient} answer={answer:?}");
            assert_eq!(fs::read(private.join("auth.json")).unwrap(),before);
        }
    }
}

#[test]
fn final_review_full_capacity_keeps_receipt_replay_and_purpose_conflict_before_capacity() {
    let root=tempfile::tempdir().unwrap();
    let private=root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let config=LocalStoreConfig{max_credentials:2,max_receipts:1,..LocalStoreConfig::default()};
    let store=LocalCredentialStore::open_with_config(&private,"auth.json",config,Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
    store.bootstrap(bootstrap()).unwrap();
    let IssueCredentialOutcome::Issued{metadata,transitions,..}=store.issue_sync(request()).unwrap() else {panic!("fresh")};
    drop(store);
    let store=LocalCredentialStore::open_with_config(&private,"auth.json",config,Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
    let before=fs::read(private.join("auth.json")).unwrap();
    let mut matching=request(); matching.credential_id="new-server-proposal".into(); matching.issued_at=150;
    let IssueCredentialOutcome::ExistingSecretUnavailable{metadata:current,transitions:original}=store.issue_sync(matching).unwrap() else {panic!("receipt not new secret")};
    assert_eq!(current,metadata);assert_eq!(original,transitions);
    let mut changed=request();changed.purpose=CredentialIssuePurpose::Product;
    assert!(matches!(store.issue_sync(changed),Err(LocalStoreError::Conflict)));
    let mut another=request();another.request_id="new-command".into();another.credential_id="new-id".into();
    assert!(matches!(store.issue_sync(another),Err(LocalStoreError::Capacity)));
    assert_eq!(fs::read(private.join("auth.json")).unwrap(),before);
}
