//! Original revocation facts share the current access snapshot, not a new history.
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
    },
    application::{
        credential_admin::{
            CredentialIssuePurpose, IssueCredentialOutcome, IssueCredentialRequest,
            RevokeCredentialRequest,
        },
        dto::*,
        origin::CredentialOrigin,
        ports::{AccessAdmission, AccessError, AccessReader, AccessSnapshot},
        revocation::{CredentialRevocation, RevocationRecord},
    },
    domain::*,
};
use std::sync::Arc;

struct Fixture {
    store: Option<LocalCredentialStore>,
    private: std::path::PathBuf,
    _root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        nessa_local_storage::create_directory(&private).unwrap();
        let store = Self::open(&private);
        let _owner = store
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "owner-member".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "owner-key".into(),
                issued_at: 10,
                expires_at: None,
                grants: vec![grant("credential.manage")],
            })
            .unwrap();
        Self {
            _root: root,
            private,
            store: Some(store),
        }
    }
    fn open(root: &std::path::Path) -> LocalCredentialStore {
        LocalCredentialStore::open(
            root,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap()
    }
    fn store(&self) -> &LocalCredentialStore {
        self.store.as_ref().unwrap()
    }
    fn reopen(&mut self) {
        drop(self.store.take());
        self.store = Some(Self::open(&self.private));
    }
    fn issue(&self, purpose: CredentialIssuePurpose) -> IssueCredentialOutcome {
        self.store()
            .issue_sync(IssueCredentialRequest {
                purpose,
                request_id: "issue-synthetic".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "synthetic-hook".into(),
                principal: PrincipalInputDto {
                    id: "hook".into(),
                    kind: PrincipalKindDto::Integration,
                },
                membership: MembershipInputDto {
                    id: "hook-member".into(),
                    principal_id: "hook".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Member,
                    state: MembershipStateDto::Active,
                },
                audience_id: "gateway".into(),
                issued_at: 20,
                expires_at: Some(100),
                grants: vec![grant("conversation.read")],
            })
            .unwrap()
    }
}
fn grant(action: &str) -> CredentialGrantDto {
    CredentialGrantDto {
        action: action.into(),
        resource: ResourceDto {
            organization_id: "org".into(),
            id: "gateway".into(),
        },
    }
}

#[test]
fn explicit_fact_is_coherent_held_original_and_survives_cold_restore() {
    ready(async {
        let mut f = Fixture::new();
        let _issued = f.issue(CredentialIssuePurpose::Webhook);
        let id = CredentialId::new("synthetic-hook").unwrap();
        let before = f.store().hold(&id).await.unwrap();
        assert_eq!(before.snapshot.revocation_fact().unwrap(), None);
        let request = RevokeCredentialRequest {
            request_id: "revoke-synthetic".into(),
            issuer_principal_id: "owner".into(),
            credential_id: id.as_str().into(),
            revoked_at: 101,
        };
        assert!(f.store().revoke_sync(request.clone()).is_err());
        drop(before);
        let outcome = f.store().revoke_sync(request.clone()).unwrap();
        let held = f.store().hold(&id).await.unwrap();
        let current = &held.snapshot;
        let fact = current.revocation_fact().unwrap().unwrap().clone();
        assert_eq!(
            fact.transition().cause(),
            &TransitionCause::Revoked(RevocationCause::Explicit)
        );
        assert_eq!(
            fact.transition().initiator(),
            &Initiator::Principal(PrincipalId::new("owner").unwrap())
        );
        assert_eq!(fact.transition().at(), 101);
        assert_eq!(fact.sequence(), outcome.revocation.sequence);
        assert_eq!(fact.revision(), outcome.revocation.revision);
        assert_eq!(fact.correlation(), Some("revoke-synthetic"));
        assert_eq!(current.origin.cause(), IssuanceCause::WebhookIssue);
        drop(held);
        let replay = f.store().revoke_sync(request).unwrap();
        assert_eq!(replay, outcome);
        f.reopen();
        let restored = f.store().read(&id).await.unwrap();
        assert_eq!(restored.revocation_fact().unwrap(), Some(&fact));
    });
}

#[test]
fn both_actual_supersessions_preserve_cause_actor_and_replacement() {
    ready(async {
        for recovery in [false, true] {
            let mut f = Fixture::new();
            let (id, replacement, kind) = if recovery {
                let _new = f
                    .store()
                    .recover_owner("owner-two".into(), &["credential.manage"], 30, None)
                    .unwrap();
                ("owner-key", "owner-two", Supersession::OwnerRecovery)
            } else {
                let _first = f
                    .store()
                    .provision_surface(
                        "panel",
                        "first".into(),
                        "surface-one".into(),
                        vec!["server.read".into()],
                        20,
                        None,
                    )
                    .unwrap();
                let _second = f
                    .store()
                    .provision_surface(
                        "panel",
                        "second".into(),
                        "surface-two".into(),
                        vec!["server.read".into()],
                        30,
                        None,
                    )
                    .unwrap();
                ("surface-one", "surface-two", Supersession::Provision)
            };
            let id = CredentialId::new(id).unwrap();
            let first = f.store().read(&id).await.unwrap();
            let fact = first.revocation_fact().unwrap().unwrap().clone();
            let expected_actor = if recovery {
                Initiator::LocalOperator
            } else {
                Initiator::Principal(PrincipalId::new("owner").unwrap())
            };
            assert_eq!(fact.transition().initiator(), &expected_actor);
            assert_eq!(
                fact.transition().cause(),
                &TransitionCause::Revoked(RevocationCause::Superseded {
                    by: CredentialId::new(replacement).unwrap(),
                    kind
                })
            );
            assert_eq!(fact.transition().at(), 30);
            f.reopen();
            assert_eq!(
                f.store()
                    .read(&id)
                    .await
                    .unwrap()
                    .revocation_fact()
                    .unwrap(),
                Some(&fact)
            );
        }
    });
}

#[test]
fn expiry_has_no_revocation_actor_and_later_fact_does_not_rewrite_issuance() {
    ready(async {
        let f = Fixture::new();
        let _issued = f.issue(CredentialIssuePurpose::Webhook);
        let id = CredentialId::new("synthetic-hook").unwrap();
        let expired = f.store().read(&id).await.unwrap();
        assert_eq!(expired.credential.expires_at(), Some(100));
        assert!(!expired.credential.is_valid_at(100));
        assert_eq!(expired.revocation_fact().unwrap(), None);
        let _revoked = f
            .store()
            .revoke_sync(RevokeCredentialRequest {
                request_id: "after-expiry".into(),
                issuer_principal_id: "owner".into(),
                credential_id: id.as_str().into(),
                revoked_at: 101,
            })
            .unwrap();
        let after = f.store().read(&id).await.unwrap();
        assert_eq!(after.origin, expired.origin);
        assert_eq!(expired.revocation_fact().unwrap(), None);
        assert_eq!(
            after.revocation_fact().unwrap().unwrap().transition().at(),
            101
        );
    });
}

fn synthetic() -> (AccessSnapshot, CredentialTransitionDto) {
    let mut credential = Credential::new(
        CredentialId::new("synthetic").unwrap(),
        PrincipalId::new("owner").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        10,
        Some(100),
        vec![],
    )
    .unwrap();
    let issued = credential
        .issued(IssuanceCause::AdminIssue, Initiator::LocalOperator)
        .unwrap();
    let origin = CredentialOrigin::restore(
        CredentialTransitionDto::record(&issued, 1, 1, None),
        &credential,
    )
    .unwrap();
    let revoked = credential
        .revoke(20, Initiator::LocalOperator)
        .unwrap()
        .unwrap();
    let record = CredentialTransitionDto::record(&revoked, 2, 2, None);
    let snapshot = AccessSnapshot {
        origin,
        revocation: Some(CredentialRevocation::restore(record.clone(), &credential).unwrap()),
        credential,
        membership: Membership::new(
            MembershipId::new("member").unwrap(),
            PrincipalId::new("owner").unwrap(),
            OrganizationId::new("org").unwrap(),
            MembershipRole::Admin,
            MembershipStatus::Active,
        ),
        revision: 2,
    };
    (snapshot, record)
}

#[test]
fn missing_mismatched_future_and_out_of_order_facts_fail_closed() {
    let (snapshot, record) = synthetic();
    assert!(snapshot.revocation_fact().unwrap().is_some());
    let mut unrevoked = snapshot.clone();
    unrevoked.credential = Credential::new(
        snapshot.credential.id().clone(),
        snapshot.credential.principal_id().clone(),
        snapshot.credential.organization_id().clone(),
        snapshot.credential.audience_id().clone(),
        10,
        Some(100),
        vec![],
    )
    .unwrap();
    assert!(unrevoked.revocation_fact().is_err());
    let issued = unrevoked
        .credential
        .issued(IssuanceCause::AdminIssue, Initiator::LocalOperator)
        .unwrap();
    assert!(CredentialRevocation::restore(
        CredentialTransitionDto::record(&issued, 1, 1, None),
        &unrevoked.credential,
    )
    .is_err());
    let mut reversed_revision = snapshot.clone();
    reversed_revision.origin = CredentialOrigin::restore(
        CredentialTransitionDto::record(&issued, 1, 3, None),
        &snapshot.credential,
    )
    .unwrap();
    reversed_revision.revision = 4;
    assert!(reversed_revision.revocation_fact().is_err());
    let mut missing = snapshot.clone();
    missing.revocation = None;
    assert_eq!(missing.revocation_fact(), Err(AccessError::Unavailable));
    let mut future = snapshot.clone();
    future.revision = 1;
    assert!(future.revocation_fact().is_err());
    let mut earlier = record.clone();
    earlier.sequence = 1;
    let mut unordered = snapshot.clone();
    unordered.revocation =
        Some(CredentialRevocation::restore(earlier, &unordered.credential).unwrap());
    assert!(unordered.revocation_fact().is_err());
    for change in [
        (|r: &mut CredentialTransitionDto| r.credential_id = "other".into())
            as fn(&mut CredentialTransitionDto),
        |r| {
            r.after.revoked_at = Some(21);
            r.at = 21;
        },
        |r| r.before = None,
        |r| r.after.expires_at = Some(99),
        |r| {
            r.before.as_mut().unwrap().issued_at = 11;
            r.after.issued_at = 11;
        },
        |r| {
            r.before.as_mut().unwrap().expires_at = Some(99);
            r.after.expires_at = Some(99);
        },
        |r| r.sequence = 0,
        |r| r.revision = 0,
        |r| r.correlation = Some(" ".into()),
        |r| r.correlation = Some("x".repeat(201)),
    ] {
        let mut changed = record.clone();
        change(&mut changed);
        assert!(CredentialRevocation::restore(changed, &snapshot.credential).is_err());
    }
}

#[test]
fn ordinary_product_keeps_one_time_secret_and_facts_are_secret_free() {
    ready(async {
        let f = Fixture::new();
        let IssueCredentialOutcome::Issued { evidence, .. } =
            f.issue(CredentialIssuePurpose::Product)
        else {
            panic!("new ordinary credential")
        };
        let secret = String::from_utf8(evidence.expose_bytes().to_vec()).unwrap();
        let id = CredentialId::new("synthetic-hook").unwrap();
        let current = f.store().read(&id).await.unwrap();
        assert_eq!(current.origin.cause(), IssuanceCause::AdminIssue);
        assert_eq!(current.revocation_fact().unwrap(), None);
        assert!(!format!("{current:?}").contains(&secret));
        assert!(matches!(
            f.issue(CredentialIssuePurpose::Product),
            IssueCredentialOutcome::ExistingSecretUnavailable { .. }
        ));
    });
}

#[test]
fn malformed_actual_revocation_history_rejects_cold_open_without_replacing_bytes() {
    ready(async {
        let mut f = Fixture::new();
        let _issued = f.issue(CredentialIssuePurpose::Webhook);
        let _revoked = f
            .store()
            .revoke_sync(RevokeCredentialRequest {
                request_id: "cold-revoke".into(),
                issuer_principal_id: "owner".into(),
                credential_id: "synthetic-hook".into(),
                revoked_at: 101,
            })
            .unwrap();
        drop(f.store.take());
        let path = f.private.join("auth.json");
        let original = std::fs::read(&path).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&original).unwrap();
        for change in [
            (|v: &mut serde_json::Value| {
                v["transitions"].as_array_mut().unwrap().pop();
            }) as fn(&mut serde_json::Value),
            |v| {
                v["transitions"].as_array_mut().unwrap().last_mut().unwrap()["initiator"] =
                    serde_json::json!({"kind":"principal","id":"missing-principal"});
            },
            |v| {
                v["transitions"].as_array_mut().unwrap().last_mut().unwrap()["credentialId"] =
                    serde_json::json!("owner-key");
            },
            |v| {
                let t = v["transitions"].as_array_mut().unwrap().last_mut().unwrap();
                t["at"] = serde_json::json!(102);
                t["after"]["revokedAt"] = serde_json::json!(102);
            },
            |v| {
                let revision = v["revision"].as_u64().unwrap();
                v["transitions"].as_array_mut().unwrap().last_mut().unwrap()["revision"] =
                    serde_json::json!(revision + 1);
            },
            |v| {
                v["transitions"].as_array_mut().unwrap().last_mut().unwrap()["cause"] =
                    serde_json::json!({"kind":"issued","cause":"webhook_issue"});
            },
        ] {
            let mut changed = document.clone();
            change(&mut changed);
            let bytes = serde_json::to_vec(&changed).unwrap();
            assert_ne!(bytes, original);
            std::fs::write(&path, &bytes).unwrap();
            assert!(LocalCredentialStore::open(
                &f.private,
                "auth.json",
                Arc::new(CedarPolicyEvaluator::new().unwrap())
            )
            .is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::write(&path, &original).unwrap();
        f.reopen();
        let snapshot = f
            .store()
            .read(&CredentialId::new("synthetic-hook").unwrap())
            .await
            .unwrap();
        assert_eq!(
            snapshot
                .revocation_fact()
                .unwrap()
                .unwrap()
                .transition()
                .initiator(),
            &Initiator::Principal(PrincipalId::new("owner").unwrap())
        );
    });
}

fn ready<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("isolated fixture unexpectedly waits"),
    }
}

#[test]
fn historical_revocation_actor_and_absent_correlation_are_not_inferred_from_receipt() {
    ready(async {
        let mut f = Fixture::new();
        let _issued = f.issue(CredentialIssuePurpose::Webhook);
        let request = RevokeCredentialRequest {
            request_id: "historical-revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "synthetic-hook".into(),
            revoked_at: 101,
        };
        let _outcome = f.store().revoke_sync(request.clone()).unwrap();
        drop(f.store.take());
        let path = f.private.join("auth.json");
        let original: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // A receipt can record a later request for an already-revoked key. It
        // does not prove who caused that original revocation. Preserve accepted
        // historical facts instead of substituting that receipt's owner.
        for (actor, expected) in [
            (
                serde_json::json!({"kind":"local_operator"}),
                Initiator::LocalOperator,
            ),
            (
                serde_json::json!({"kind":"principal","id":"hook"}),
                Initiator::Principal(PrincipalId::new("hook").unwrap()),
            ),
        ] {
            let mut document = original.clone();
            let record = document["transitions"]
                .as_array_mut()
                .unwrap()
                .last_mut()
                .unwrap();
            record["initiator"] = actor;
            record["correlation"] = serde_json::Value::Null;
            std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
            f.reopen();
            let snapshot = f
                .store()
                .read(&CredentialId::new("synthetic-hook").unwrap())
                .await
                .unwrap();
            let fact = snapshot.revocation_fact().unwrap().unwrap();
            assert_eq!(fact.transition().initiator(), &expected);
            assert_eq!(fact.correlation(), None);
            assert_eq!(
                f.store()
                    .revoke_sync(request.clone())
                    .unwrap()
                    .revocation
                    .initiator,
                document["transitions"]
                    .as_array()
                    .unwrap()
                    .last()
                    .and_then(|r| serde_json::from_value::<CredentialTransitionDto>(r.clone()).ok())
                    .unwrap()
                    .initiator
            );
            drop(f.store.take());
        }
    });
}

#[test]
fn retained_decoder_reuses_canonical_validation_without_claiming_current_binding() {
    ready(async {
        let f = Fixture::new();
        let _issued = f.issue(CredentialIssuePurpose::Webhook);
        let request = RevokeCredentialRequest {
            request_id: "retained-revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "synthetic-hook".into(),
            revoked_at: 101,
        };
        let outcome = f.store().revoke_sync(request).unwrap();
        let id = CredentialId::new("synthetic-hook").unwrap();
        let held = f.store().hold(&id).await.unwrap();
        let original = held.snapshot.revocation_fact().unwrap().unwrap();
        assert_eq!(
            RevocationRecord::decode(outcome.revocation.clone()).unwrap(),
            *original.retained_record()
        );
        let owner = f
            .store()
            .read(&CredentialId::new("owner-key").unwrap())
            .await
            .unwrap();
        assert!(!original.matches(&owner.credential, owner.revision));
        assert_eq!(
            CredentialRevocation::restore(outcome.revocation.clone(), &owner.credential),
            Err(AccessError::IdentityMismatch)
        );
        for mutation in 0..6 {
            let mut row = outcome.revocation.clone();
            match mutation {
                0 => row.sequence = 0,
                1 => row.revision = 0,
                2 => row.correlation = Some(" ".into()),
                3 => row.correlation = Some("x".repeat(201)),
                4 => row.at += 1,
                5 => {
                    row.cause = TransitionCauseDto::Issued {
                        cause: IssuanceCauseDto::WebhookIssue,
                    }
                }
                _ => unreachable!(),
            }
            assert_eq!(RevocationRecord::decode(row), Err(AccessError::Unavailable));
        }
        assert_eq!(held.snapshot.revocation_fact().unwrap(), Some(original));
    });
}
