from pathlib import Path
p=Path('/tmp/independent-total-context-review/crates/nessa-server/tests/webhooks/infrastructure/outbox.rs')
p.write_text(p.read_text()+r'''
#[tokio::test]
async fn independent_source_refusal_replaces_original_cursor_actor() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let held=f.audit.with_extension("held");
    std::fs::rename(&f.audit,&held).unwrap();
    assert_eq!(f.store.queue(replaced_source(f.record(2))).await,Err(OutboxError::Unavailable));
    std::fs::rename(&held,&f.audit).unwrap();
    let id=edit_evidence(&f,"webhook_delivery","source_changed",|r| {
        r["initiator"]=json!("restart");
        r["atMs"]=json!(9999);
    });
    let reopened=reopen_store(&mut f).await.unwrap();
    assert!(reopened.due(3000).await.unwrap().is_empty());
    let stop:String=reopened.owner.connection.lock().unwrap().query_row("SELECT reset_stop FROM cursors",[],|r|r.get(0)).unwrap();
    assert_eq!(codec::source_stop(&stop).unwrap().initiator,QueueInitiator::Live);
    let published:Value=serde_json::from_str(&std::fs::read_to_string(f.audit.join(format!("{id}.json"))).unwrap()).unwrap();
    assert_eq!(published["initiator"],"restart");
    assert_eq!(published["atMs"],9999);
}
''')
