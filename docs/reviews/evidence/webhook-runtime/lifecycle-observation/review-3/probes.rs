use super::*;

fn initial_issue(f: &Fixture) -> Value {
 f.audits().into_iter().find(|v|v["cause"]=="credential_issue").unwrap()
}
fn add_initial(f:&Fixture,issue:&Value,published:bool) {
 f.store.owner.connection.lock().unwrap().execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,?4)",params![issue["recordId"].as_str().unwrap(),f.registration.id.to_string(),issue.to_string(),i64::from(published)]).unwrap();
}
fn snapshot(f:&Fixture)->Vec<String> {
 let c=f.store.owner.connection.lock().unwrap();
 ["SELECT json_group_array(json_array(id,active,stop_cause,stop_initiator,stop_at)) FROM registrations",
 "SELECT json_group_array(json_array(id,state)) FROM deliveries",
 "SELECT json_group_array(json_array(id,record,published)) FROM audit",
 "SELECT json_group_array(json_array(id,audit)) FROM retirements",
 "SELECT json_group_array(json_array(registration,conversation,scope,position,reset_required)) FROM cursors"].iter().map(|s|c.query_row(s,[],|r|r.get(0)).unwrap()).collect()
}
#[tokio::test]
async fn independent_owed_issue_expiry_refuses_first_cold_open_before_bytes_or_files_change() {
 for reader in [ExpiryObserver::Owner(PrincipalId::new("known-owner").unwrap()),ExpiryObserver::Restart,ExpiryObserver::Delivery(Uuid::new_v4())] {
  let mut f=Fixture::new().await; let issue=initial_issue(&f);
  f.store.observe_expiry(f.registration.target.credential.clone(),reader,2001).await.unwrap();
  add_initial(&f,&issue,false);
  let before=snapshot(&f); let held=f.audit.with_extension("held");std::fs::rename(&f.audit,&held).unwrap();
  assert_eq!(f.store.due(3000).await.err(),Some(OutboxError::Invalid));assert_eq!(snapshot(&f),before);assert!(!f.audit.exists());std::fs::rename(held,&f.audit).unwrap();
  let placeholder=Fixture::new().await;drop(std::mem::replace(&mut f.store,placeholder.store));
  let bytes=std::fs::read(&f.path).unwrap();let files=std::fs::read_dir(&f.audit).unwrap().count();
  assert!(matches!(SqlOutbox::open(&f.path,f.audit.clone(),f.clock.clone()),Err(OutboxError::Invalid)));
  assert_eq!(std::fs::read(&f.path).unwrap(),bytes);assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(),files);
  let c=Connection::open(&f.path).unwrap();c.execute("DELETE FROM audit WHERE id=?1",[issue["recordId"].as_str().unwrap()]).unwrap();drop(c);
  f.store=SqlOutbox::open(&f.path,f.audit.clone(),f.clock.clone()).unwrap();f.store.stop(f.registration.target.credential.clone(),StopReason::Expired{observed_by:ExpiryObserver::Restart},3001).await.unwrap();
 }
}
#[tokio::test]
async fn diagnostic_retained_cursor_history_proves_initial_issue_completion() {
 let mut f=Fixture::new().await;let issue=initial_issue(&f);
 f.store.queue(f.record(1)).await.unwrap();
 assert_eq!(f.store.queue(replaced_source(f.record(2))).await,Err(OutboxError::ResetRequired));
 let cursor=f.audits().into_iter().find(|v|v["kind"]=="webhook_cursor").unwrap();
 let reset=cursor["sourceResetId"].as_str().unwrap();
 {
  let c=f.store.owner.connection.lock().unwrap();
  // Keep the actual published cursor record and original retained reset fact.
  c.execute("DELETE FROM cursors",[]).unwrap();c.execute("DELETE FROM deliveries",[]).unwrap();
  assert_eq!(c.query_row("SELECT refs FROM source_reset_facts WHERE id=?1",[reset],|r|r.get::<_,i64>(0)).unwrap(),1);
  assert_eq!(restoration::validate(&c),Ok(()));
 }
 add_initial(&f,&issue,false);
 {
  let c=f.store.owner.connection.lock().unwrap();
  assert_eq!(c.query_row(restoration::ISSUANCE_WORK,[f.registration.id.to_string()],|r|r.get::<_,bool>(0)).unwrap(),false);
  assert_eq!(restoration::validate(&c),Ok(()),"diagnostic acceptance, not a fix");
 }
 let reopened=reopen_store(&mut f).await.unwrap();f.store=reopened;
 let held=f.audit.with_extension("held");std::fs::rename(&f.audit,&held).unwrap();
 assert_eq!(f.store.due(3000).await.err(),Some(OutboxError::Unavailable));
 std::fs::rename(held,&f.audit).unwrap();f.store.due(3000).await.unwrap();
 let c=f.store.owner.connection.lock().unwrap();assert_eq!(c.query_row("SELECT count(*) FROM audit WHERE published=0",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[tokio::test]
async fn required_initial_rejection_with_only_retained_source_history() {
 let mut f=Fixture::new().await;let issue=initial_issue(&f);
 f.store.queue(f.record(1)).await.unwrap();
 assert_eq!(f.store.queue(replaced_source(f.record(2))).await,Err(OutboxError::ResetRequired));
 {
  let c=f.store.owner.connection.lock().unwrap();
  c.execute("DELETE FROM cursors",[]).unwrap();c.execute("DELETE FROM deliveries",[]).unwrap();
  assert_eq!(restoration::validate(&c),Ok(()));
 }
 add_initial(&f,&issue,false);
 {
  let c=f.store.owner.connection.lock().unwrap();
  assert_eq!(restoration::validate(&c),Err(OutboxError::Invalid));
 }
 let before=snapshot(&f);let held=f.audit.with_extension("held");std::fs::rename(&f.audit,&held).unwrap();
 assert_eq!(f.store.due(3000).await.err(),Some(OutboxError::Invalid));assert_eq!(snapshot(&f),before);assert!(!f.audit.exists());std::fs::rename(held,&f.audit).unwrap();
 let placeholder=Fixture::new().await;drop(std::mem::replace(&mut f.store,placeholder.store));
 let bytes=std::fs::read(&f.path).unwrap();let files=std::fs::read_dir(&f.audit).unwrap().count();
 assert!(matches!(SqlOutbox::open(&f.path,f.audit.clone(),f.clock.clone()),Err(OutboxError::Invalid)));
 assert_eq!(std::fs::read(&f.path).unwrap(),bytes);assert_eq!(std::fs::read_dir(&f.audit).unwrap().count(),files);
 let c=Connection::open(&f.path).unwrap();c.execute("DELETE FROM audit WHERE id=?1",[issue["recordId"].as_str().unwrap()]).unwrap();drop(c);
 f.store=SqlOutbox::open(&f.path,f.audit.clone(),f.clock.clone()).unwrap();f.store.due(3000).await.unwrap();
}

#[tokio::test]
async fn diagnostic_published_initial_issue_is_also_acknowledgement_proof() {
 let mut f=Fixture::new().await;let original=initial_issue(&f);
 {
  let c=f.store.owner.connection.lock().unwrap();
  assert_eq!(c.query_row("SELECT published FROM audit WHERE id=?1",[original["recordId"].as_str().unwrap()],|r|r.get::<_,i64>(0)).unwrap(),1);
 }
 let mut contradiction=original.clone();contradiction["recordId"]=json!(Uuid::new_v4().to_string());
 add_initial(&f,&contradiction,false);
 {
  let c=f.store.owner.connection.lock().unwrap();
  assert_eq!(c.query_row(restoration::ISSUANCE_WORK,[f.registration.id.to_string()],|r|r.get::<_,bool>(0)).unwrap(),false);
  assert_eq!(restoration::validate(&c),Ok(()),"diagnostic acceptance, not a fix");
 }
 let original_files=f.audits().len();
 let reopened=reopen_store(&mut f).await.unwrap();f.store=reopened;
 f.store.due(3000).await.unwrap();
 assert_eq!(f.audits().len(),original_files+1);
 assert!(f.audits().iter().any(|v|v["recordId"]==contradiction["recordId"]));
}
