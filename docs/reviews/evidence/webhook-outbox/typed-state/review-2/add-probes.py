from pathlib import Path
p=Path('/tmp/independent-webhook-final-review/crates/nessa-server/tests/webhooks/infrastructure/outbox.rs')
p.write_text(p.read_text()+r'''

#[tokio::test]
async fn independent_registration_stop_actor_disagrees_with_retained_fact() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let q=f.store.due(2000).await.unwrap().remove(0).delivery;
    let s=q.admit(7,2000).unwrap();
    f.store.transition(&q,&s,DeliveryCause::Admission,2000).await.unwrap();
    let held=f.audit.with_extension("held");
    std::fs::rename(&f.audit,&held).unwrap();
    assert_eq!(f.store.stop(f.registration.target.credential.clone(),StopReason::Revoked{initiator:PrincipalId::new("original-revoker").unwrap()},2001).await,Err(OutboxError::Unavailable));
    std::fs::rename(&held,&f.audit).unwrap();
    let (id,text):(String,String)=f.store.owner.connection.lock().unwrap().query_row("SELECT id,record FROM audit WHERE delivery IS NULL AND json_extract(record,'$.cause')='credential_revoked'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let mut record:Value=serde_json::from_str(&text).unwrap();
    record["initiator"]=json!("replacement-revoker");
    f.store.owner.connection.lock().unwrap().execute("UPDATE audit SET record=?1,published=0 WHERE id=?2",params![record.to_string(),id]).unwrap();
    let reopened=reopen_store(&mut f).await.unwrap();
    let (actor,):(String,)=reopened.owner.connection.lock().unwrap().query_row("SELECT stop_initiator FROM registrations",[],|r|Ok((r.get(0)?,))).unwrap();
    assert_eq!(actor,"original-revoker");
    assert!(reopened.due(3000).await.unwrap().is_empty());
    let published:Value=serde_json::from_str(&std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(published["initiator"],"replacement-revoker");
}

#[tokio::test]
async fn independent_registration_issue_target_disagrees_with_retained_fact() {
    let mut f=Fixture::new().await;
    let (id,text):(String,String)=f.store.owner.connection.lock().unwrap().query_row("SELECT id,record FROM audit WHERE delivery IS NULL AND json_extract(record,'$.cause')='credential_issue'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    std::fs::remove_file(f.audit.join(format!("{id}.json"))).unwrap();
    let mut record:Value=serde_json::from_str(&text).unwrap();
    record["principal"]=json!("replacement-principal");
    record["owner"]=json!("replacement-owner");
    record["initiator"]=json!("replacement-owner");
    f.store.owner.connection.lock().unwrap().execute("UPDATE audit SET record=?1,published=0 WHERE id=?2",params![record.to_string(),id]).unwrap();
    let reopened=reopen_store(&mut f).await.unwrap();
    let registration=reopened.registration(f.registration.target.credential.clone()).await.unwrap().unwrap();
    assert_eq!(registration.target.principal.as_str(),"synthetic-hook");
    let published:Value=serde_json::from_str(&std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(published["principal"],"replacement-principal");
}
''')
