from pathlib import Path
p=Path('/tmp/independent-total-context-review/crates/nessa-server/tests/webhooks/infrastructure/outbox.rs')
p.write_text(p.read_text()+r'''
#[tokio::test]
async fn independent_capture_needs_an_original_source_reset() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let q=f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending=q.admit(7,2000).unwrap();
    f.store.transition(&q,&sending,DeliveryCause::Admission,2000).await.unwrap();
    f.store.owner.connection.lock().unwrap().execute("UPDATE deliveries SET source_stop=?1",[codec::source_stop_text(codec::SourceStop{initiator:QueueInitiator::Restart,at_ms:9999}).unwrap()]).unwrap();
    let reopened=reopen_store(&mut f).await.unwrap();
    assert_eq!(reopened.owner.connection.lock().unwrap().query_row("SELECT reset_required FROM cursors",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    reopened.recover(3000).await.unwrap();
    let recovered=f.audits().into_iter().find(|r|r["cause"]=="restart").unwrap();
    assert_eq!(recovered["sourceStop"]["at_ms"],9999);
}
''')
