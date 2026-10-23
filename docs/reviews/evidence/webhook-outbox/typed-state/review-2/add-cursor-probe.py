from pathlib import Path
p=Path('/tmp/independent-webhook-final-review/crates/nessa-server/tests/webhooks/infrastructure/outbox.rs')
p.write_text(p.read_text()+r'''
#[tokio::test]
async fn independent_cursor_audit_checkpoint_disagrees_with_retained_fact() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let q=f.store.due(2000).await.unwrap().remove(0).delivery;
    let s=q.admit(7,2000).unwrap();
    f.store.transition(&q,&s,DeliveryCause::Admission,2000).await.unwrap();
    let mut changed=f.record(2);
    changed.scope=Scope::new(changed.scope.receiver().clone(),changed.scope.origin().clone(),changed.scope.stream().clone(),Id::new("incarnation-b").unwrap(),changed.scope.schema().clone(),changed.scope.access_epoch().clone());
    let held=f.audit.with_extension("held");
    std::fs::rename(&f.audit,&held).unwrap();
    assert_eq!(f.store.queue(changed).await,Err(OutboxError::Unavailable));
    std::fs::rename(&held,&f.audit).unwrap();
    let (id,text):(String,String)=f.store.owner.connection.lock().unwrap().query_row("SELECT id,record FROM audit WHERE json_extract(record,'$.kind')='webhook_cursor'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let mut record:Value=serde_json::from_str(&text).unwrap();
    record["priorPosition"]=json!(999);
    f.store.owner.connection.lock().unwrap().execute("UPDATE audit SET record=?1,published=0 WHERE id=?2",params![record.to_string(),id]).unwrap();
    let reopened=reopen_store(&mut f).await.unwrap();
    let (position,):(i64,)=reopened.owner.connection.lock().unwrap().query_row("SELECT position FROM cursors",[],|r|Ok((r.get(0)?,))).unwrap();
    assert_eq!(position,1);
    assert!(reopened.due(3000).await.unwrap().is_empty());
    let published:Value=serde_json::from_str(&std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(published["priorPosition"],999);
}
''')
