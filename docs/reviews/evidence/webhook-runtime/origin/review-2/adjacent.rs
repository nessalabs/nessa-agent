
#[tokio::test]
async fn independent_ambiguous_revoked_or_expired_original_retains_exact_history_on_repeated_recovery() {
    for revoked in [false, true] {
        let f = Registered::new();
        let proposed = f.proposed();
        let reserved = f.store.reserve(proposed.clone()).await.unwrap();
        let issue = reserved.issue;
        let id = CredentialId::new(&issue.credential_id).unwrap();
        drop(f.auth.issue_sync(issue.clone()).unwrap());
        if revoked {
            f.auth.revoke_sync(RevokeCredentialRequest { request_id: "independent-revoke".into(), issuer_principal_id: "owner".into(), credential_id: id.as_str().into(), revoked_at: 11 }).unwrap();
        } else { f.clock.0.store(100_000, Ordering::SeqCst); }
        let private = f._root.path().join("private"); drop(f.auth);
        let path = private.join("auth.json");
        let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        saved["transitions"][1]["cause"]["cause"] = json!("admin_issue");
        saved["transitions"][1]["initiator"] = json!({"kind":"local_operator"});
        saved["transitions"][1]["correlation"] = Value::Null;
        let bytes = serde_json::to_vec(&saved).unwrap();std::fs::write(&path,&bytes).unwrap();
        let auth = LocalCredentialStore::open(&private,"auth.json",Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
        let original = auth.read(&id).await.unwrap().origin;
        let command=RegisterWebhook{credentials:&auth,admission:&auth,intents:&f.store,clock:f.clock.as_ref()};
        for _ in 0..2 {
            assert!(matches!(auth.issue(issue.clone()).await, Err(CredentialAdminError::OriginRecoveryRequired)));
            assert_eq!(command.execute(proposed.clone()).await, Err(RegistrationFailure::OriginRecoveryRequired));
            assert_eq!(auth.read(&id).await.unwrap().origin, original);
            assert_eq!(std::fs::read(&path).unwrap(),bytes);
            let connection=f.store.owner.connection.lock().unwrap();
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM registrations",[],|row|row.get::<_,i64>(0)).unwrap(),0);
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM registration_intents",[],|row|row.get::<_,i64>(0)).unwrap(),1);
        }
    }
}

#[tokio::test]
async fn independent_changed_command_conflicts_before_historical_recovery_without_mutating_auth() {
    let f=Registered::new();let intent=f.store.reserve(f.proposed()).await.unwrap();let issue=intent.issue;
    drop(f.auth.issue_sync(issue.clone()).unwrap());let private=f._root.path().join("private");drop(f.auth);
    let path=private.join("auth.json");let mut saved:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["transitions"][1]["cause"]["cause"]=json!("admin_issue");saved["transitions"][1]["correlation"]=Value::Null;
    let bytes=serde_json::to_vec(&saved).unwrap();std::fs::write(&path,&bytes).unwrap();
    let auth=LocalCredentialStore::open(&private,"auth.json",Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
    let mut changed=issue.clone();changed.expires_at=Some(99);
    assert!(matches!(auth.issue(changed).await,Err(CredentialAdminError::Conflict)));
    let mut product=issue.clone();product.purpose=CredentialIssuePurpose::Product;
    assert!(matches!(auth.issue(product).await,Err(CredentialAdminError::Conflict)));
    assert!(matches!(auth.issue(issue).await,Err(CredentialAdminError::OriginRecoveryRequired)));
    assert_eq!(std::fs::read(path).unwrap(),bytes);
}

#[tokio::test]
async fn independent_ordinary_product_matching_receipt_remains_secret_unavailable_after_cold_restore() {
    let f=Registered::new();let mut request=f.proposed().issue;request.purpose=CredentialIssuePurpose::Product;
    let IssueCredentialOutcome::Issued{metadata,transitions,evidence}=f.auth.issue_sync(request.clone()).unwrap() else {panic!("new product issue");};
    let private=f._root.path().join("private");drop(f.auth);
    let auth=LocalCredentialStore::open(&private,"auth.json",Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
    let IssueCredentialOutcome::ExistingSecretUnavailable{metadata:retry,transitions:retry_transitions}=auth.issue(request).await.unwrap()else{panic!("one-time secret replay");};
    assert_eq!(metadata,retry);assert_eq!(transitions,retry_transitions);
    let policy=CedarPolicyEvaluator::new().unwrap();assert!(AuthenticateSession{verifier:&auth,access:&auth,clock:f.clock.as_ref(),policy:&policy}.execute(&evidence,&AudienceId::new("gateway").unwrap()).await.is_ok());
}
