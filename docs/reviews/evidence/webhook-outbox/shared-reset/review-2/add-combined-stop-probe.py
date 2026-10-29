from pathlib import Path
p=Path('/tmp/independent-webhook-round2/crates/nessa-server/tests/webhooks/infrastructure/outbox/source_reset.rs')
s=p.read_text()+r'''

#[tokio::test]
async fn independent_source_and_credential_causes_survive_shared_cleanup_and_retirement() {
    let mut f=Fixture::new().await;
    let sending=captured_sender(&f).await;
    let reset=current_reset(&f);
    f.store.stop(f.registration.target.credential.clone(),StopReason::Revoked{
        initiator: PrincipalId::new("second-independent-revoker").unwrap()
    },2001).await.unwrap();
    let retry=sending.observed(Observation::Retryable,3000).unwrap();
    f.store.transition(&sending,&retry,DeliveryCause::Response,3000).await.unwrap();
    let refused=f.audits().into_iter().find(|r|r["deliveryId"]==sending.id().to_string()&&r["after"]=="not_sent").unwrap();
    assert_eq!(refused["cause"],"credential_revoked");
    assert_eq!(refused["initiator"],"second-independent-revoker");
    assert_eq!(refused["sourceResetId"],reset.to_string());
    assert_eq!(refused["sourceStop"]["initiator"],"live");
    assert_eq!(refused["sourceStop"]["at_ms"],1002);
    assert_eq!(refused["credentialStop"]["initiator"],"second-independent-revoker");
    assert_eq!(refused["credentialStop"]["atMs"],2001);
    let reopened=reopen_store(&mut f).await.unwrap();
    assert!(reopened.due(4000).await.unwrap().is_empty());
    let c=reopened.owner.connection.lock().unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM source_reset_facts",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(c.query_row("SELECT count(*) FROM source_reset_members",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert_eq!(c.query_row("SELECT count(*) FROM registrations",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
'''
p.write_text(s)
