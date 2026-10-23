#[tokio::test]
async fn independent_revoked_webhook_cold_restore_retains_original_and_refuses_both_entries() {
    let f = Registered::new();
    let issue = f.proposed().issue;
    let id = CredentialId::new(&issue.credential_id).unwrap();
    let IssueCredentialOutcome::Issued { evidence, .. } = f.auth.issue_sync(issue).unwrap() else { panic!() };
    let original = f.auth.read(&id).await.unwrap().origin;
    f.auth.revoke_sync(RevokeCredentialRequest { request_id:"independent-stop".into(),issuer_principal_id:"owner".into(),credential_id:id.as_str().into(),revoked_at:11 }).unwrap();
    let private = f._root.path().join("private"); drop(f.auth);
    let auth=LocalCredentialStore::open(&private,"auth.json",Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
    let snap=auth.read(&id).await.unwrap(); assert_eq!(snap.origin,original);assert_eq!(snap.credential.revoked_at(),Some(11));
    let policy=CedarPolicyEvaluator::new().unwrap();let audience=AudienceId::new("gateway").unwrap();
    assert!(AuthenticateSession{verifier:&auth,access:&auth,clock:f.clock.as_ref(),policy:&policy}.execute(&evidence,&audience).await.is_err());
    assert_eq!(ResumeSession{verifier:&OpaqueSession(id),access:&auth,clock:f.clock.as_ref(),policy:&policy}.execute(&SessionEvidence::new(b"private-test".to_vec()).unwrap(),&audience).await.unwrap_err(),AccessError::CredentialRevoked);
}
#[tokio::test]
async fn independent_historical_different_existing_actor_and_nonreceipt_correlation_remain_valid() {
    let f=Registered::new();let mut issue=f.proposed().issue;issue.purpose=CredentialIssuePurpose::Product;let id=CredentialId::new(&issue.credential_id).unwrap();
    let IssueCredentialOutcome::Issued{evidence,..}=f.auth.issue_sync(issue).unwrap()else{panic!()};let private=f._root.path().join("private");drop(f.auth);
    let path=private.join("auth.json");let mut saved:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["transitions"][1]["initiator"]=json!({"kind":"principal","id":"hook"});saved["transitions"][1]["correlation"]=json!("legacy-original-command");
    let bytes=serde_json::to_vec(&saved).unwrap();std::fs::write(&path,&bytes).unwrap();let auth=LocalCredentialStore::open(&private,"auth.json",Arc::new(CedarPolicyEvaluator::new().unwrap())).unwrap();
    let origin=auth.read(&id).await.unwrap().origin;assert_eq!(origin.correlation(),Some("legacy-original-command"));assert_eq!(origin.transition().initiator(),&nessa_auth::domain::Initiator::Principal(PrincipalId::new("hook").unwrap()));
    let policy=CedarPolicyEvaluator::new().unwrap();assert!(AuthenticateSession{verifier:&auth,access:&auth,clock:f.clock.as_ref(),policy:&policy}.execute(&evidence,&AudienceId::new("gateway").unwrap()).await.is_ok());assert_eq!(std::fs::read(path).unwrap(),bytes);
}
#[test]
fn independent_additional_invalid_restored_relationships_fail_without_rewrite() {
    let changes:[fn(&mut Value);6]=[
        |v|v["transitions"][1]["revision"]=json!(100),
        |v|v["transitions"][1]["revision"]=json!(0),
        |v|v["transitions"][1]["after"]["expiresAt"]=json!(99),
        |v|v["transitions"][1]["correlation"]=json!(""),
        |v|v["transitions"][1]["initiator"]=json!({"kind":"local_operator"}),
        |v|{let mut r=v["issueReceipts"][0].clone();r["requestId"]=json!("another-command");v["issueReceipts"].as_array_mut().unwrap().push(r);},
    ];
    for (i,change) in changes.into_iter().enumerate(){let f=Registered::new();drop(f.auth.issue_sync(f.proposed().issue).unwrap());let private=f._root.path().join("private");drop(f.auth);let path=private.join("auth.json");let mut v:Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();change(&mut v);let bytes=serde_json::to_vec(&v).unwrap();std::fs::write(&path,&bytes).unwrap();assert!(LocalCredentialStore::open(&private,"auth.json",Arc::new(CedarPolicyEvaluator::new().unwrap())).is_err(),"case {i}");assert_eq!(std::fs::read(path).unwrap(),bytes);assert_eq!(f.count("registrations"),0);}
}
