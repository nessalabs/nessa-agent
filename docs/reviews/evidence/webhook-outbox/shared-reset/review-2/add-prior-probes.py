from pathlib import Path
p=Path('/tmp/independent-webhook-round2/crates/nessa-server/tests/webhooks/infrastructure/outbox/source_reset.rs')
s=p.read_text()+r'''

#[tokio::test]
async fn independent_later_retry_cleanup_preserves_original_fact_after_cursor_gone() {
    let mut f = Fixture::new().await;
    let sending = captured_sender(&f).await;
    let reset = current_reset(&f);
    {
        let c = f.store.owner.connection.lock().unwrap();
        c.execute("DELETE FROM cursors", []).unwrap();
    }
    f.store.due(2500).await.unwrap();
    let reopened = reopen_store(&mut f).await.unwrap();
    f.clock.0.store(500, Ordering::SeqCst);
    let retry = sending.observed(Observation::Retryable, 3000).unwrap();
    reopened.transition(&sending, &retry, DeliveryCause::Response, 3000).await.unwrap();
    let record = f.audits().into_iter().find(|r| r["cause"]=="source_changed" && r["deliveryId"]==sending.id().to_string()).unwrap();
    assert_eq!(record["sourceResetId"], reset.to_string());
    assert_eq!(record["atMs"], 500);
    assert_eq!(record["sourceStop"]["at_ms"], 1002);
    assert_eq!(record["sourceStop"]["initiator"], "live");
    drop(reopened);
    let once_more = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
    assert!(once_more.due(5000).await.unwrap().is_empty());
}

#[tokio::test]
async fn independent_cross_chat_existing_reset_reference_is_rejected_before_publication() {
    let mut f = Fixture::new().await;
    let sending = captured_sender(&f).await;
    let first = current_reset(&f);
    let another_chat = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut record = f.record(1);
    record.conversation = another_chat.clone();
    record.scope = Scope::new(record.scope.receiver().clone(), record.scope.origin().clone(), Id::new(another_chat.to_string()).unwrap(),record.scope.incarnation().clone(),record.scope.schema().clone(),record.scope.access_epoch().clone());
    let second_id = f.store.queue(record).await.unwrap();
    let second_before = f.store.due(3000).await.unwrap().into_iter().find(|r| r.delivery.id()==second_id).unwrap().delivery;
    let second_sending = second_before.admit(7,3000).unwrap();
    f.store.transition(&second_before,&second_sending,DeliveryCause::Admission,3000).await.unwrap();
    let mut change = replaced_source(f.record(2));
    change.conversation = another_chat.clone();
    change.scope = Scope::new(change.scope.receiver().clone(),change.scope.origin().clone(),Id::new(another_chat.to_string()).unwrap(),change.scope.incarnation().clone(),change.scope.schema().clone(),change.scope.access_epoch().clone());
    assert_eq!(f.store.queue(change).await,Err(OutboxError::ResetRequired));
    let retry = sending.observed(Observation::Retryable,4000).unwrap();
    f.store.transition(&sending,&retry,DeliveryCause::Response,4000).await.unwrap();
    let second: String = f.store.owner.connection.lock().unwrap().query_row("SELECT source_reset FROM cursors WHERE conversation=?1",[another_chat.to_string()],|r|r.get(0)).unwrap();
    assert_ne!(first.to_string(),second);
    let id = edit_evidence(&f,"webhook_delivery","source_changed",|r| {r["sourceResetId"]=json!(second.clone());});
    f.store.owner.connection.lock().unwrap().execute("UPDATE audit SET source_reset=?1 WHERE id=?2",params![second,id]).unwrap();
    let before = f.audits();
    {
        let mut c = f.store.owner.connection.lock().unwrap();
        assert_eq!(publication::publish(&mut c,&f.audit),Err(OutboxError::Invalid));
    }
    assert_eq!(f.audits(),before);
    assert_eq!(reopen_store(&mut f).await.err(),Some(OutboxError::Invalid));
}

#[tokio::test]
async fn independent_source_membership_survives_complete_physical_source_cycle() {
    let mut f=Fixture::new().await;
    let original=captured_sender(&f).await;
    let first=current_reset(&f);
    f.store.due(3000).await.unwrap();
    f.store.owner.connection.lock().unwrap().execute("DELETE FROM cursors",[]).unwrap();
    let replacement = replaced_source(f.record(1));
    f.store.queue(replacement).await.unwrap();
    assert_eq!(f.store.queue(f.record(2)).await,Err(OutboxError::ResetRequired));
    f.store.due(3000).await.unwrap();
    f.store.owner.connection.lock().unwrap().execute("DELETE FROM cursors",[]).unwrap();
    let mut cycle=f.record(2);
    cycle.expected_after=0;
    cycle.position=1;
    // Original incarnation/position1 is already owned by the in-flight delivery.
    // Use a new record at position2 by first retaining a cursor at position1.
    {
        let c=f.store.owner.connection.lock().unwrap();
        c.execute("INSERT INTO cursors(registration,conversation,scope,position) VALUES(?1,?2,?3,1)",params![f.registration.id.to_string(),f.conversation.to_string(),codec::scope_text(&f.record(1).scope).unwrap()]).unwrap();
    }
    f.store.queue(f.record(2)).await.unwrap();
    assert_eq!(f.store.queue(replaced_source(f.record(3))).await,Err(OutboxError::ResetRequired));
    let latest=current_reset(&f);
    assert_ne!(first,latest);
    let reopened=reopen_store(&mut f).await.unwrap();
    reopened.recover(4000).await.unwrap();
    let r=f.audits().into_iter().find(|r|r["cause"]=="restart"&&r["deliveryId"]==original.id().to_string()).unwrap();
    assert_eq!(r["sourceResetId"],first.to_string());
    assert_eq!(r["sourceStop"]["at_ms"],1002);
}
'''
p.write_text(s)
