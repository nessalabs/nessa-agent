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
async fn demonstrate_stopped_state_with_unacknowledged_observation_is_accepted() {
    let mut f = Fixture::new().await;
    let held = f.audit.with_extension("held");
    std::fs::rename(&f.audit,&held).unwrap();
    assert_eq!(f.store.observe_expiry(f.registration.target.credential.clone(),ExpiryObserver::Restart,2001).await,Err(OutboxError::Unavailable));
    {
        let c=f.store.owner.connection.lock().unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM audit WHERE published=0",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        // Both individual states/facts are valid, but this phase cannot follow
        // the retained owed observation: stop requires its prior exact ack.
        c.execute("UPDATE registrations SET active=0",[]).unwrap();
        assert!(restoration::validate(&c).is_ok());
    }
    std::fs::rename(held,&f.audit).unwrap();
    cold_reopen(&mut f); // Unexpected acceptance before any owed publication.
    f.store.due(3000).await.unwrap();
    let records=f.audits();
    assert!(records.iter().any(|v|v["cause"]=="credential_expiry_observed"));
    assert!(records.iter().any(|v|v["cause"]=="drained"));
    assert!(!records.iter().any(|v|v["cause"]=="credential_expired"));
}

#[tokio::test]
async fn demonstrate_stopped_queued_after_observation_ack_is_accepted() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    f.store.observe_expiry(f.registration.target.credential.clone(),ExpiryObserver::Restart,2001).await.unwrap();
    {
        let c=f.store.owner.connection.lock().unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM audit WHERE delivery IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        c.execute("UPDATE registrations SET active=0",[]).unwrap();
        assert!(restoration::validate(&c).is_ok());
    }
    cold_reopen(&mut f);
    assert!(f.store.due(3000).await.unwrap().is_empty());
    let records=f.audits();
    assert!(records.iter().any(|v|v["kind"]=="webhook_delivery"&&v["cause"]=="credential_expired"));
    assert!(!records.iter().any(|v|v["kind"]=="webhook_registration"&&v["cause"]=="credential_expired"));
}

#[tokio::test]
async fn late_retry_after_genuine_stop_and_lost_response_ack_is_valid() {
    let mut f=Fixture::new().await;
    f.store.queue(f.record(1)).await.unwrap();
    let queued=f.store.due(1000).await.unwrap().remove(0).delivery;
    let sending=queued.admit(7,2000).unwrap();
    f.store.transition(&queued,&sending,DeliveryCause::Admission,2000).await.unwrap();
    f.store.stop(f.registration.target.credential.clone(),StopReason::Expired {observed_by:ExpiryObserver::Restart},2001).await.unwrap();
    let held=f.audit.with_extension("held");std::fs::rename(&f.audit,&held).unwrap();
    let retry=sending.observed(Observation::Retryable,2002).unwrap();
    assert_eq!(f.store.transition(&sending,&retry,DeliveryCause::Response,2002).await,Err(OutboxError::Unavailable));
    {
        let c=f.store.owner.connection.lock().unwrap();
        assert_eq!(c.query_row("SELECT phase FROM deliveries",[],|r|r.get::<_,String>(0)).unwrap(),"retry");
        assert!(restoration::validate(&c).is_ok());
    }
    std::fs::rename(held,&f.audit).unwrap();cold_reopen(&mut f);
    assert!(f.store.due(4000).await.unwrap().is_empty());
    let records=f.audits();
    assert!(records.iter().any(|v|v["cause"]=="response"&&v["after"]=="retry"));
    assert!(records.iter().filter(|v|v["cause"]=="credential_expired").all(|v|v["initiator"]=="restart"));
}

#[tokio::test]
async fn demonstrate_retired_state_accepts_reintroduced_owed_observation() {
    let mut f=Fixture::new().await;
    f.store.observe_expiry(f.registration.target.credential.clone(),ExpiryObserver::Restart,2001).await.unwrap();
    let observation=std::fs::read_dir(&f.audit).unwrap().map(|p| std::fs::read_to_string(p.unwrap().path()).unwrap()).find(|text|serde_json::from_str::<Value>(text).unwrap()["cause"]=="credential_expiry_observed").unwrap();
    let value:Value=serde_json::from_str(&observation).unwrap();
    let held=f.audit.with_extension("held");std::fs::rename(&f.audit,&held).unwrap();
    assert_eq!(f.store.stop(f.registration.target.credential.clone(),StopReason::Expired {observed_by:ExpiryObserver::Restart},3000).await,Err(OutboxError::Unavailable));
    {
        let c=f.store.owner.connection.lock().unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM registrations",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        c.execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,0)",params![value["recordId"].as_str().unwrap(),f.registration.id.to_string(),observation]).unwrap();
        assert!(restoration::validate(&c).is_ok());
    }
    std::fs::rename(held,&f.audit).unwrap();cold_reopen(&mut f);
    assert!(f.store.due(4000).await.unwrap().is_empty());
}
