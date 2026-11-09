//! Secret-free acknowledgement and actual retained issuance purpose.
use super::*;
use nessa_auth::application::{
    credential_admin::{CredentialAdmin, RevokeCredentialOutcome},
    dto::CredentialMetadataDto,
    ports::{
        AccessError, AccessReader, CredentialEvidence, CredentialVerifier, PortFuture,
        SessionEvidence, SessionVerifier, VerifiedSessionCredential,
    },
    session::{AuthenticateSession, ResumeSession},
};
use nessa_auth::domain::{AudienceId, IssuanceCause};

struct OpaqueSession(CredentialId);
impl SessionVerifier for OpaqueSession {
    fn verify_session<'a>(
        &'a self,
        _: &'a SessionEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedSessionCredential> {
        Box::pin(async move {
            Ok(VerifiedSessionCredential {
                credential_id: self.0.clone(),
            })
        })
    }
}

#[tokio::test]
async fn actual_webhook_origin_refuses_bearer_and_resumed_product_entry_after_restart() {
    for webhook in [true, false] {
        let f = Registered::new();
        let mut issue = f.proposed().issue;
        if !webhook {
            issue.purpose = CredentialIssuePurpose::Product;
        }
        let IssueCredentialOutcome::Issued {
            evidence, metadata, ..
        } = f.auth.issue_sync(issue).unwrap()
        else {
            panic!("fresh synthetic issuance")
        };
        let private = f._root.path().join("private");
        drop(f.auth);
        let auth = LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        let policy = CedarPolicyEvaluator::new().unwrap();
        let audience = AudienceId::new("gateway").unwrap();
        let id = CredentialId::new(metadata.id).unwrap();
        // Proof verification is deliberately separate from session permission.
        assert_eq!(
            auth.verify(&evidence, &audience)
                .await
                .unwrap()
                .credential_id,
            id
        );
        let session = AuthenticateSession {
            verifier: &auth,
            access: &auth,
            clock: f.clock.as_ref(),
            policy: &policy,
        }
        .execute(&evidence, &audience)
        .await;
        let opaque = OpaqueSession(id.clone());
        let resumed = ResumeSession {
            verifier: &opaque,
            access: &auth,
            clock: f.clock.as_ref(),
            policy: &policy,
        }
        .execute(
            &SessionEvidence::new(b"isolated-session".to_vec()).unwrap(),
            &audience,
        )
        .await;
        if webhook {
            assert_eq!(session.unwrap_err(), AccessError::Denied);
            assert_eq!(resumed.unwrap_err(), AccessError::Denied);
        } else {
            assert!(session.is_ok());
            assert!(resumed.is_ok());
        }
        let before = auth.read(&id).await.unwrap().origin;
        auth.revoke_sync(RevokeCredentialRequest {
            request_id: "stop".into(),
            issuer_principal_id: "owner".into(),
            credential_id: id.as_str().into(),
            revoked_at: 11,
        })
        .unwrap();
        let after = auth.read(&id).await.unwrap();
        assert_eq!(after.origin, before);
        assert!(after.credential.revoked_at().is_some());
    }
}

struct SecretReply<'a>(&'a LocalCredentialStore);
impl CredentialAdmin for SecretReply<'_> {
    fn issue<'a>(
        &'a self,
        request: IssueCredentialRequest,
    ) -> PortFuture<'a, IssueCredentialOutcome, CredentialAdminError> {
        Box::pin(async move {
            let issued = self.0.issue(request).await?;
            Ok(match issued {
                IssueCredentialOutcome::Issued {
                    metadata,
                    transitions,
                    ..
                } => IssueCredentialOutcome::Issued {
                    metadata,
                    transitions,
                    evidence: CredentialEvidence::new(b"isolated-unneeded-bearer-marker".to_vec())
                        .unwrap(),
                },
                other => other,
            })
        })
    }
    fn list<'a>(
        &'a self,
        request: ListCredentialsRequest,
    ) -> PortFuture<'a, Vec<CredentialMetadataDto>, CredentialAdminError> {
        self.0.list(request)
    }
    fn revoke<'a>(
        &'a self,
        request: RevokeCredentialRequest,
    ) -> PortFuture<'a, RevokeCredentialOutcome, CredentialAdminError> {
        self.0.revoke(request)
    }
}

#[tokio::test]
async fn bearer_never_enters_registration_result_database_or_audit_and_replay_is_identical() {
    let mut f = Registered::new();
    let reply = SecretReply(&f.auth);
    let mut command = f.command(&f.store);
    command.credentials = &reply;
    let initial = command.execute(f.proposed()).await.unwrap();
    assert!(!format!("{initial:?}").contains("isolated-unneeded-bearer-marker"));
    f.reopen();
    let retry = f.command(&f.store).execute(f.proposed()).await.unwrap();
    assert_eq!(initial, retry);
    for entry in std::fs::read_dir(&f.audit).unwrap() {
        assert!(!std::fs::read_to_string(entry.unwrap().path())
            .unwrap()
            .contains("isolated-unneeded-bearer-marker"));
    }
    let database = std::fs::read(&f.path).unwrap();
    assert!(!database
        .windows(b"isolated-unneeded-bearer-marker".len())
        .any(|bytes| bytes == b"isolated-unneeded-bearer-marker"));
}

#[tokio::test]
async fn ambiguous_historical_admin_origin_is_preserved_and_requires_recovery() {
    let changes: [fn(&mut Value); 5] = [
        |_| {},
        |v| v["transitions"][1]["correlation"] = Value::Null,
        |v| {
            v["transitions"][1]["initiator"] = json!({"kind":"local_operator"});
            v["transitions"][1]["correlation"] = Value::Null;
        },
        |v| v["transitions"][1]["initiator"] = json!({"kind":"principal","id":"hook"}),
        |v| v["transitions"][1]["correlation"] = json!("original-nonreceipt-command"),
    ];
    for change in changes {
        let f = Registered::new();
        let proposed = f.proposed();
        let intent = f.store.reserve(proposed.clone()).await.unwrap();
        let request = intent.issue;
        drop(f.auth.issue_sync(request.clone()).unwrap());
        let private = f._root.path().join("private");
        drop(f.auth);
        let path = private.join("auth.json");
        let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        saved["transitions"][1]["cause"]["cause"] = json!("admin_issue");
        change(&mut saved);
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let original = std::fs::read(&path).unwrap();
        let auth = LocalCredentialStore::open(
            &private,
            "auth.json",
            Arc::new(CedarPolicyEvaluator::new().unwrap()),
        )
        .unwrap();
        let id = CredentialId::new(&request.credential_id).unwrap();
        assert_eq!(
            auth.read(&id).await.unwrap().origin.cause(),
            IssuanceCause::AdminIssue
        );
        assert!(matches!(
            auth.issue(request).await,
            Err(CredentialAdminError::OriginRecoveryRequired)
        ));
        let command = RegisterWebhook {
            credentials: &auth,
            admission: &auth,
            intents: &f.store,
            stops: &f.store,
            clock: f.clock.as_ref(),
        };
        for _ in 0..2 {
            assert_eq!(
                command.execute(proposed.clone()).await,
                Err(RegistrationFailure::OriginRecoveryRequired)
            );
            let connection = f.store.owner.connection.lock().unwrap();
            for (table, expected) in [("registrations", 0), ("registration_intents", 1)] {
                assert_eq!(
                    connection
                        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                            .get::<_, i64>(0))
                        .unwrap(),
                    expected
                );
            }
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }
}

#[tokio::test]
async fn retiring_private_configuration_cannot_restore_product_session_authority() {
    let f = Registered::new();
    let registered = f.command(&f.store).execute(f.proposed()).await.unwrap();
    let id = CredentialId::new(registered.credential.id).unwrap();
    let original = f.auth.read(&id).await.unwrap().origin;
    f.store
        .stop(
            id.clone(),
            StopReason::Revoked {
                initiator: Initiator::Principal(PrincipalId::new("owner").unwrap()),
                cause: RevocationCause::Explicit,
            },
            11_000,
        )
        .await
        .unwrap();
    assert_eq!(
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM registrations", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.store
            .owner
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM registration_intents", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        0
    );
    assert_eq!(f.auth.read(&id).await.unwrap().origin, original);
    let policy = CedarPolicyEvaluator::new().unwrap();
    let verifier = OpaqueSession(id);
    let result = ResumeSession {
        verifier: &verifier,
        access: &f.auth,
        clock: f.clock.as_ref(),
        policy: &policy,
    }
    .execute(
        &SessionEvidence::new(b"test-only-session".to_vec()).unwrap(),
        &AudienceId::new("gateway").unwrap(),
    )
    .await;
    assert_eq!(result.unwrap_err(), AccessError::Denied);
}

#[test]
fn restored_webhook_origin_cannot_carry_product_write_or_malformed_history() {
    let changes: [fn(&mut Value); 10] = [
        |v| v["credentials"][1]["metadata"]["grants"][0]["action"] = json!("conversation.write"),
        |v| v["principals"][1]["kind"] = json!("human"),
        |v| v["transitions"][1]["credentialId"] = json!("dangling"),
        |v| v["transitions"][1]["after"]["issuedAt"] = json!(9),
        |v| v["transitions"][1]["sequence"] = json!(1),
        |v| v["transitions"][1]["initiator"]["id"] = json!("absent-owner"),
        |v| v["transitions"][1]["correlation"] = json!("substituted"),
        |v| v["transitions"][1]["initiator"]["id"] = json!("hook"),
        |v| {
            v["issueReceipts"].as_array_mut().unwrap().clear();
        },
        |v| {
            v["transitions"].as_array_mut().unwrap().pop();
        },
    ];
    for (index, change) in changes.into_iter().enumerate() {
        let f = Registered::new();
        let _issued = f.auth.issue_sync(f.proposed().issue).unwrap();
        let private = f._root.path().join("private");
        drop(f.auth);
        let path = private.join("auth.json");
        let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        change(&mut saved);
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(
            LocalCredentialStore::open(
                &private,
                "auth.json",
                Arc::new(CedarPolicyEvaluator::new().unwrap())
            )
            .is_err(),
            "case {index}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[tokio::test]
async fn historical_product_actor_and_absent_command_correlation_are_not_rewritten() {
    let f = Registered::new();
    let mut issue = f.proposed().issue;
    issue.purpose = CredentialIssuePurpose::Product;
    let id = CredentialId::new(issue.credential_id.clone()).unwrap();
    drop(f.auth.issue_sync(issue.clone()).unwrap());
    let private = f._root.path().join("private");
    drop(f.auth);
    let path = private.join("auth.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["transitions"][1]["initiator"] = json!({"kind":"local_operator"});
    saved["transitions"][1]["correlation"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let original = saved["transitions"][1].clone();
    let auth = LocalCredentialStore::open(
        &private,
        "auth.json",
        Arc::new(CedarPolicyEvaluator::new().unwrap()),
    )
    .unwrap();
    // Product replay keeps its existing unavailable-evidence outcome. It must
    // not acquire the webhook-only recovery classification or rewrite history.
    let bytes = std::fs::read(&path).unwrap();
    assert!(matches!(
        auth.issue(issue).await,
        Err(CredentialAdminError::Unavailable)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let before = auth.read(&id).await.unwrap().origin;
    assert_eq!(before.transition().initiator(), &Initiator::LocalOperator);
    assert_eq!(before.correlation(), None);
    auth.revoke_sync(RevokeCredentialRequest {
        request_id: "revoke-historical".into(),
        issuer_principal_id: "owner".into(),
        credential_id: id.as_str().into(),
        revoked_at: 11,
    })
    .unwrap();
    assert_eq!(auth.read(&id).await.unwrap().origin, before);
    let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(after["transitions"][1], original);
}
