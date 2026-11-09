from pathlib import Path
p=Path('/tmp/independent-total-context-review/crates/nessa-server/tests/webhooks/infrastructure/outbox.rs')
p.write_text(p.read_text()+r'''
#[tokio::test]
async fn independent_sending_capture_disagrees_with_original_cursor_reset() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let q=f.store.due(2000).await.unwrap().remove(0).delivery;
    let sending=q.admit(7,2000).unwrap();
    f.store.transition(&q,&sending,DeliveryCause::Admission,2000).await.unwrap();
    assert_eq!(f.store.queue(replaced_source(f.record(2))).await,Err(OutboxError::ResetRequired));
    {
        let c=f.store.owner.connection.lock().unwrap();
        let cursor_stop:String=c.query_row("SELECT reset_stop FROM cursors",[],|r|r.get(0)).unwrap();
        assert_eq!(codec::source_stop(&cursor_stop).unwrap().initiator,QueueInitiator::Live);
        c.execute("UPDATE deliveries SET source_stop=?1",[codec::source_stop_text(codec::SourceStop{initiator:QueueInitiator::Restart,at_ms:9999}).unwrap()]).unwrap();
    }
    let reopened=reopen_store(&mut f).await.unwrap();
    reopened.recover(3000).await.unwrap();
    let (cursor_stop,delivery_stop):(String,String)={let c=reopened.owner.connection.lock().unwrap();(c.query_row("SELECT reset_stop FROM cursors",[],|r|r.get(0)).unwrap(),c.query_row("SELECT source_stop FROM deliveries",[],|r|r.get(0)).unwrap())};
    assert_eq!(codec::source_stop(&cursor_stop).unwrap().initiator,QueueInitiator::Live);
    assert_eq!(codec::source_stop(&delivery_stop).unwrap().initiator,QueueInitiator::Restart);
    let recovered=f.audits().into_iter().find(|r|r["cause"]=="restart").unwrap();
    assert_eq!(recovered["sourceStop"]["initiator"],"restart");
    assert_eq!(recovered["sourceStop"]["at_ms"],9999);
}
''')
