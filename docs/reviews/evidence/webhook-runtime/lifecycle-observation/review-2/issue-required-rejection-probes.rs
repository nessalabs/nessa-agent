use super::*;

fn cold_reopen(f: &mut Fixture) {
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let temporary = SqlOutbox::open(&private.join("outbox.db"), private.join("audit"), f.clock.clone()).unwrap();
    drop(std::mem::replace(&mut f.store, temporary));
    let restored = SqlOutbox::open(&f.path, f.audit.clone(), f.clock.clone()).unwrap();
    drop(std::mem::replace(&mut f.store, restored));
}

#[tokio::test]
async fn all_original_readers_survive_ack_loss_restart_and_later_revocation() {
    for kind in 0..3 {
        let mut f = Fixture::new().await;
        let queued = f.record(1);
        let id = queued.id;
        f.store.queue(queued).await.unwrap();
        let before = f.store.due(1000).await.unwrap().remove(0).delivery;
        let sending = before.admit(7, 2000).unwrap();
        f.store.transition(&before, &sending, DeliveryCause::Admission, 2000).await.unwrap();
        let first = match kind {
            0 => ExpiryObserver::Owner(PrincipalId::new("actual-observing-owner").unwrap()),
            1 => ExpiryObserver::Delivery(id),
            _ => ExpiryObserver::Restart,
        };
        let expected = match &first {
            ExpiryObserver::Owner(p) => format!("owner:{}", p.as_str()),
            ExpiryObserver::Delivery(id) => format!("delivery:{id}"),
            ExpiryObserver::Restart => "restart".into(),
        };
        let held = f.audit.with_extension("held");
        std::fs::rename(&f.audit, &held).unwrap();
        assert_eq!(f.store.observe_expiry(f.registration.target.credential.clone(), first, 2001).await, Err(OutboxError::Unavailable));
        std::fs::rename(held, &f.audit).unwrap();
        cold_reopen(&mut f);
        f.store.observe_expiry(f.registration.target.credential.clone(), ExpiryObserver::Owner(PrincipalId::new("later-reader").unwrap()), 2002).await.unwrap();
        // Losing the response while merely observed keeps recovery distinct
        // from the later stop. Restart never invents a second HTTP effect.
        f.store.recover(2500).await.unwrap();
        f.store.stop(f.registration.target.credential.clone(), StopReason::Revoked { initiator: PrincipalId::new("actual-later-revoker").unwrap() }, 3000).await.unwrap();
        assert!(f.store.due(4000).await.unwrap().is_empty());
        let records = f.audits();
        assert_eq!(records.iter().filter(|v| v["cause"]=="credential_expiry_observed").count(),1);
        for cause in ["credential_expiry_observed","credential_expired"] {
            for v in records.iter().filter(|v| v["cause"]==cause) {
                assert_eq!(v["initiator"], expected);
                assert_eq!(v["atMs"], 2001);
            }
        }
        let retired = records.iter().find(|v| v["cause"]=="drained").unwrap();
        assert_eq!(retired["initiator"],expected);
        assert_eq!(retired["stopAtMs"],2001);
        cold_reopen(&mut f);
        assert!(f.store.due(4000).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn observation_then_known_response_then_stop_keeps_effect_and_cause_distinct() {
    let mut f = Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let before = f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending = before.admit(7,2000).unwrap();
    f.store.transition(&before,&sending,DeliveryCause::Admission,2000).await.unwrap();
    f.store.observe_expiry(f.registration.target.credential.clone(),ExpiryObserver::Restart,2001).await.unwrap();
    let acknowledged = sending.observed(Observation::Success,2002).unwrap();
    f.store.transition(&sending,&acknowledged,DeliveryCause::Response,2002).await.unwrap();
    cold_reopen(&mut f);
    f.store.stop(f.registration.target.credential.clone(),StopReason::Expired { observed_by:ExpiryObserver::Owner(PrincipalId::new("later-owner").unwrap()) },3000).await.unwrap();
    let records=f.audits();
    assert!(records.iter().any(|v|v["cause"]=="response"&&v["atMs"]==2002));
    assert!(records.iter().filter(|v|v["cause"]=="credential_expired").all(|v|v["initiator"]=="restart"&&v["atMs"]==2001));
    cold_reopen(&mut f);
    assert!(f.store.due(4000).await.unwrap().is_empty());
}


#[tokio::test]
async fn probe_reintroduced_owed_issue_after_expiry_observation() {
    let mut f=Fixture::new().await;
    let issue=f.audits().into_iter().find(|v|v["cause"]=="credential_issue").unwrap();
    f.store.observe_expiry(f.registration.target.credential.clone(),ExpiryObserver::Restart,2001).await.unwrap();
    let id=issue["recordId"].as_str().unwrap();
    {
        let c=f.store.owner.connection.lock().unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM audit WHERE delivery IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        c.execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,0)",params![id,f.registration.id.to_string(),issue.to_string()]).unwrap();
        assert_eq!(restoration::validate(&c),Err(OutboxError::Invalid),"owed issuance prerequisite contradicts retained ExpiryObserved");
    }
    cold_reopen(&mut f); // cold restore also accepts the provable prerequisite contradiction.
    let held=f.audit.with_extension("held");
    std::fs::rename(&f.audit,&held).unwrap();
    // If complete phase/ack correlation were enforced this would be Invalid;
    // current implementation reaches the immutable sink (Unavailable).
    assert_eq!(f.store.due(3000).await.err(),Some(OutboxError::Unavailable));
    std::fs::rename(held,&f.audit).unwrap();
    f.store.due(3000).await.unwrap();
    let c=f.store.owner.connection.lock().unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM audit WHERE published=0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

fn snapshot(f:&Fixture)->Vec<String> {
 let c=f.store.owner.connection.lock().unwrap();
 ["SELECT json_group_array(json_array(id,active,stop_cause,stop_initiator,stop_at)) FROM registrations",
 "SELECT json_group_array(json_array(id,state)) FROM deliveries",
 "SELECT json_group_array(json_array(id,record,published)) FROM audit",
 "SELECT json_group_array(json_array(id,audit)) FROM retirements",
 "SELECT json_group_array(json_array(registration,conversation,scope,position,reset_required)) FROM cursors"].iter().map(|s|c.query_row(s,[],|r|r.get(0)).unwrap()).collect()
}
fn cold_refuse_repair(f:&mut Fixture, repair:impl FnOnce(&Connection)) {
 let placeholder_root=tempfile::tempdir().unwrap();
 let private=placeholder_root.path().join("private");
 nessa_local_storage::create_directory(&private).unwrap();
 let placeholder=SqlOutbox::open(&private.join("outbox.db"),private.join("audit"),f.clock.clone()).unwrap();
 drop(std::mem::replace(&mut f.store,placeholder));
 let before=std::fs::read(&f.path).unwrap();
 let files=std::fs::read_dir(&f.audit).unwrap().count();
 assert!(matches!(SqlOutbox::open(&f.path,f.audit.clone(),f.clock.clone()),Err(OutboxError::Invalid)));
 assert_eq!(std::fs::read(&f.path).unwrap(),before,"cold refusal rewrote authoritative database");
 assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(),files);
 let c=Connection::open(&f.path).unwrap();repair(&c);drop(c);
 cold_reopen(f);
}
#[tokio::test]
async fn independent_m1_three_forms_all_readers_reject_before_effects_then_recover() {
 for kind in 0..3 {
  for form in 0..3 {
   let mut f=Fixture::new().await;
   let reader=match kind {0=>ExpiryObserver::Restart,1=>ExpiryObserver::Owner(PrincipalId::new("first-owner").unwrap()),_=>ExpiryObserver::Delivery(Uuid::new_v4())};
   if form==1 {f.store.queue(f.record(1)).await.unwrap();}
   let held=f.audit.with_extension("held");
   if form==0 {std::fs::rename(&f.audit,&held).unwrap();}
   let result=f.store.observe_expiry(f.registration.target.credential.clone(),reader,2001).await;
   assert_eq!(result,if form==0 {Err(OutboxError::Unavailable)}else{Ok(())});
   let mut reintroduced=String::new();
   if form==2 {
    let record=f.audits().into_iter().find(|v|v["cause"]=="credential_expiry_observed").unwrap();
    reintroduced=record["recordId"].as_str().unwrap().into();
    std::fs::rename(&f.audit,&held).unwrap();
    assert_eq!(f.store.stop(f.registration.target.credential.clone(),StopReason::Expired{observed_by:ExpiryObserver::Restart},3000).await,Err(OutboxError::Unavailable));
    let c=f.store.owner.connection.lock().unwrap();
    c.execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,0)",params![reintroduced,f.registration.id.to_string(),record.to_string()]).unwrap();
   }else{
    f.store.owner.connection.lock().unwrap().execute("UPDATE registrations SET active=0",[]).unwrap();
   }
   let before=snapshot(&f);
   let files=if f.audit.exists(){std::fs::read_dir(&f.audit).unwrap().count()}else{0};
   assert_eq!(f.store.due(4000).await.err(),Some(OutboxError::Invalid),"reader {kind}, form {form}");
   assert_eq!(snapshot(&f),before);
   if form==1 {assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(),files);}else{assert!(!f.audit.exists());std::fs::rename(&held,&f.audit).unwrap();}
   cold_refuse_repair(&mut f,|c|{if form==2 {c.execute("DELETE FROM audit WHERE id=?1",[reintroduced]).unwrap();}else{c.execute("UPDATE registrations SET active=1",[]).unwrap();}});
   f.store.due(4000).await.unwrap();
   if form!=2 {f.store.stop(f.registration.target.credential.clone(),StopReason::Expired{observed_by:ExpiryObserver::Restart},5000).await.unwrap();}
   assert!(f.store.due(6000).await.unwrap().is_empty());
  }
 }
}
