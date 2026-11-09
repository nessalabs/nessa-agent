use super::*;
use nessa_auth::application::{ports::AccessReader, session::AuthenticateSession};
use nessa_auth::domain::{AudienceId, IssuanceCause};

#[tokio::test]
async fn independent_expiry_retirement_preserves_auth_origin_and_another_registration_after_lost_ack() {
    let mut f = Registered::new();
    let proposed = f.proposed();
    let first_id = CredentialId::new(&proposed.issue.credential_id).unwrap();
    let first = f.command(&f.store).execute(proposed.clone()).await.unwrap();
    assert_eq!(first, f.command(&f.store).execute(proposed.clone()).await.unwrap());
    let original = f.auth.read(&first_id).await.unwrap().origin;
    assert_eq!(original.cause(), IssuanceCause::WebhookIssue);
    let mut second = f.proposed();
    second.issue.request_id = "independent-second-register".into();
    second.issue.principal.id = "hook-second".into();
    second.issue.membership.principal_id = "hook-second".into();
    second.issue.membership.id = "hook-second-member".into();
    second.issue.expires_at = Some(200);
    let second = f.command(&f.store).execute(second).await.unwrap();
    f.clock.0.store(100_000, Ordering::SeqCst);
    f.store.owner.connection.lock().unwrap().execute_batch(
        "CREATE TRIGGER review_hold BEFORE UPDATE OF published ON audit
         WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;",
    ).unwrap();
    assert_eq!(f.store.stop(first_id.clone(), StopReason::Expired { observed_by: ExpiryObserver::Lifecycle }, 100_000).await, Err(OutboxError::Conflict));
    assert_eq!(f.count("registrations"), 1);
    assert_eq!(f.count("registration_intents"), 1);
    f.store.owner.connection.lock().unwrap().execute_batch("DROP TRIGGER review_hold").unwrap();
    f.reopen();
    assert!(f.store.due(100_001).await.unwrap().is_empty());
    let remaining=f.store.registration(CredentialId::new(&second.credential.id).unwrap()).await.unwrap().unwrap();
    assert_eq!(remaining.id, second.registration_id);
    assert_eq!(f.auth.read(&first_id).await.unwrap().origin, original);
    let policy=CedarPolicyEvaluator::new().unwrap();
    let issue = f.auth.issue_sync({let mut r=f.proposed().issue;r.purpose=CredentialIssuePurpose::Product;r.request_id="independent-product".into();r.principal.id="product-member".into();r.membership.principal_id="product-member".into();r.membership.id="product-member-membership".into();r.issued_at=100;r.expires_at=Some(200);r}).unwrap();
    let IssueCredentialOutcome::Issued {evidence,..}=issue else {panic!("product one-time bearer")};
    assert!(AuthenticateSession { policy:&policy, verifier:&f.auth,access:&f.auth,clock:f.clock.as_ref() }.execute(&evidence,&AudienceId::new("gateway").unwrap()).await.is_ok());
    let logs=std::fs::read_dir(&f.audit).unwrap().map(|e|serde_json::from_str::<Value>(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap()).collect::<Vec<_>>();
    assert!(logs.iter().any(|a|a["cause"]=="drained"&&a["registration"]==first.registration_id.to_string()&&a["initiator"]=="lifecycle"&&a["stopAtMs"]==100_000));
    assert!(!format!("{first:?}").contains("recipient.test"));
    assert!(!format!("{first:?}").contains("evidence"));
}
