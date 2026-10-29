use super::*;
fn original(f: &Fixture) -> Value { f.audits().into_iter().find(|r|r["cause"]=="credential_issue").unwrap() }
fn add(f:&Fixture,v:&Value,published:bool) {
 f.store.owner.connection.lock().unwrap().execute("INSERT INTO audit(id,registration,record,published) VALUES(?1,?2,?3,?4)",params![v["recordId"].as_str().unwrap(),v["registration"].as_str().unwrap(),v.to_string(),i64::from(published)]).unwrap();
}
#[tokio::test]
async fn independent_reset_only_proof_is_scoped_away_from_a_new_pending_registration() {
 for file_written in [false,true] {
  let mut f=Fixture::new().await;
  f.store.queue(f.record(1)).await.unwrap();
  assert_eq!(f.store.queue(replaced_source(f.record(2))).await,Err(OutboxError::ResetRequired));
  {
   let c=f.store.owner.connection.lock().unwrap();c.execute("DELETE FROM cursors",[]).unwrap();c.execute("DELETE FROM deliveries",[]).unwrap();
   assert_eq!(c.query_row("SELECT count(*) FROM source_reset_facts",[],|r|r.get::<_,i64>(0)).unwrap(),1);
   assert_eq!(restoration::validate(&c),Ok(()));
  }
  let mut next=f.registration.clone();next.id=Uuid::new_v4();next.target.credential=CredentialId::new("independent-new-key").unwrap();next.target.principal=PrincipalId::new("independent-new-principal").unwrap();
  let held=f.audit.with_extension("held");
  if file_written { f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER lose_ack BEFORE UPDATE OF published ON audit BEGIN SELECT RAISE(IGNORE); END;").unwrap(); }
  else {std::fs::rename(&f.audit,&held).unwrap();}
  assert_eq!(f.store.install(next.clone(),1000).await,Err(if file_written {OutboxError::Conflict}else{OutboxError::Unavailable}));
  assert_eq!(restoration::validate(&f.store.owner.connection.lock().unwrap()),Ok(()));
  if file_written {f.store.owner.connection.lock().unwrap().execute_batch("DROP TRIGGER lose_ack").unwrap();}
  else{std::fs::rename(held,&f.audit).unwrap();}
  f.registration=next;f.store=reopen_store(&mut f).await.unwrap();f.store.queue(f.record(1)).await.unwrap();
  assert_eq!(f.store.due(1001).await.unwrap().len(),1);
 }
}
#[tokio::test]
async fn independent_mismatched_initial_actor_does_not_establish_progress() {
 let mut f=Fixture::new().await;let mut issue=original(&f);issue["recordId"]=json!(Uuid::new_v4().to_string());issue["owner"]=json!("substituted-owner");issue["initiator"]=json!("substituted-owner");
 add(&f,&issue,false);
 let held=f.audit.with_extension("held");std::fs::rename(&f.audit,&held).unwrap();
 assert_eq!(f.store.due(3000).await.err(),Some(OutboxError::Invalid));assert!(!f.audit.exists());std::fs::rename(held,&f.audit).unwrap();
 assert!(matches!(reopen_store(&mut f).await,Err(OutboxError::Invalid)));
}
#[tokio::test]
async fn independent_published_cursor_actor_still_requires_original_reset_fact() {
 let mut f=Fixture::new().await;f.store.queue(f.record(1)).await.unwrap();
 assert_eq!(f.store.queue(replaced_source(f.record(2))).await,Err(OutboxError::ResetRequired));
 let mut cursor=f.audits().into_iter().find(|r|r["kind"]=="webhook_cursor").unwrap();
 {
  let c=f.store.owner.connection.lock().unwrap();c.execute("DELETE FROM cursors",[]).unwrap();c.execute("DELETE FROM deliveries",[]).unwrap();
  assert_eq!(restoration::validate(&c),Ok(()));
  cursor["initiator"]=json!("restart");
  c.execute("UPDATE audit SET record=?1 WHERE id=?2",params![cursor.to_string(),cursor["recordId"].as_str().unwrap()]).unwrap();
  assert_eq!(restoration::validate(&c),Err(OutboxError::Invalid));
 }
 assert!(matches!(reopen_store(&mut f).await,Err(OutboxError::Invalid)));
}
