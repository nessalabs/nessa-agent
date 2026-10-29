//! Independent SQL equivalence/transaction probes. Metadata rows intentionally
//! do not claim authenticated domain history; final async probe uses real owners.
use super::*;
use crate::webhooks::infrastructure::outbox::journal::{TRIM_TERMINALS,TRIM_REGISTRATION_AUDIT};
const OLD: &str="DELETE FROM deliveries WHERE id IN (SELECT id FROM (SELECT d.id,row_number() OVER (PARTITION BY registration ORDER BY d.rowid DESC) AS retained FROM deliveries d WHERE phase NOT IN ('queued','sending','retry')) WHERE retained>?1) AND NOT EXISTS(SELECT 1 FROM audit a WHERE a.delivery=deliveries.id AND a.published=0)";
fn rows(c:&Connection,sql:&str)->Vec<String>{let mut s=c.prepare(sql).unwrap();s.query_map([],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap()}
fn count(c:&Connection,t:&str)->i64{c.query_row(&format!("SELECT count(*) FROM {t}"),[],|r|r.get(0)).unwrap()}
fn metadata()->Connection{
 let c=Connection::open_in_memory().unwrap();c.execute_batch(SCHEMA).unwrap();c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
 for n in 0..4 {let r=format!("r{n}");c.execute("INSERT INTO registration_intents VALUES(?1,?1,?1,'{}','https://synthetic.test',zeroblob(32),0)",[&r]).unwrap();c.execute("INSERT INTO registrations VALUES(?1,?1,?1,'org','owner',0,'https://synthetic.test',zeroblob(32),1,NULL,NULL,NULL)",[&r]).unwrap();} c
}
fn delivery(c:&Connection,id:&str,r:&str,pos:i64,phase:&str){c.execute("INSERT INTO deliveries(id,registration,conversation,incarnation,position,scope,body,state) VALUES(?1,?2,'chat','inc',?3,'{}',x'00',json_object('phase',?4,'due_at',1))",params![id,r,pos,phase]).unwrap();}
fn audit(c:&Connection,id:&str,d:Option<&str>,r:&str,p:i64,reset:Option<&str>){c.execute("INSERT INTO audit(id,delivery,registration,record,published,source_reset) VALUES(?1,?2,?3,'{}',?4,?5)",params![id,d,r,p,reset]).unwrap();}
#[test]
fn independent_multiregistration_differential_cutoff(){
 for seed in 0..24u64 {
  let mut a=metadata();let mut b=metadata();b.execute_batch("DROP TRIGGER delivery_audit_retire").unwrap();
  let keep=if seed%6==0{1024}else{3};let n=if keep==1024{2000}else{20};let mut rng=seed+1;
  let ta=a.transaction().unwrap();let tb=b.transaction().unwrap();
  for i in 1..=n {for reg in 0..4 {
   rng=rng.wrapping_mul(6364136223846793005).wrapping_add(1);let x=(rng>>32)%10;
   let phase=match x{0=>"queued",1=>"sending",2=>"retry",3=>"unknown",4=>"refused",5=>"exhausted",_=>"sent"};let id=format!("{reg}-{i}");let r=format!("r{reg}");
   for c in [&*ta,&*tb] {delivery(c,&id,&r,i,phase);audit(c,&format!("published-{id}"),Some(&id),&r,1,None);if x%3==0{audit(c,&format!("owed-{id}"),Some(&id),&r,0,None);}}
  }}
  for c in [&*ta,&*tb] {audit(c,"registration-published",None,"r0",1,None);audit(c,"registration-owed",None,"r1",0,None);}
  ta.commit().unwrap();tb.commit().unwrap();
  a.execute(TRIM_TERMINALS,[keep-1]).unwrap();a.execute(TRIM_REGISTRATION_AUDIT,[]).unwrap();
  b.execute(OLD,[keep]).unwrap();b.execute("DELETE FROM audit WHERE published=1 AND (delivery IS NULL OR NOT EXISTS(SELECT 1 FROM deliveries d WHERE d.id=audit.delivery))",[]).unwrap();
  assert_eq!(rows(&a,"SELECT id FROM deliveries ORDER BY id"),rows(&b,"SELECT id FROM deliveries ORDER BY id"),"seed {seed}");
  assert_eq!(rows(&a,"SELECT id FROM audit ORDER BY id"),rows(&b,"SELECT id FROM audit ORDER BY id"),"seed {seed}");
  assert!(rows(&a,"PRAGMA foreign_key_check").is_empty());
 }
}
#[test]
fn independent_reference_release_and_rollback_are_atomic(){
 let mut c=metadata();delivery(&c,"old","r0",1,"sent");delivery(&c,"new","r0",2,"sent");
 c.execute("INSERT INTO source_reset_facts(id,registration,fact) VALUES('reset','r0','{}')",[]).unwrap();
 c.execute("INSERT INTO cursors VALUES('r0','chat','{}',2,1,'reset')",[]).unwrap();
 c.execute("INSERT INTO source_reset_members VALUES('old','reset',1,'{}')",[]).unwrap();
 audit(&c,"published",Some("old"),"r0",1,Some("reset"));audit(&c,"owed",Some("old"),"r0",0,Some("reset"));
 assert_eq!(c.query_row("SELECT refs FROM source_reset_facts",[],|r|r.get::<_,i64>(0)).unwrap(),4);
 c.execute("DELETE FROM cursors",[]).unwrap();assert_eq!(c.execute(TRIM_TERMINALS,[0]).unwrap(),0);
 for commit in [false,true] {
  let tx=c.transaction().unwrap();tx.execute("UPDATE audit SET published=1 WHERE id='owed'",[]).unwrap();
  assert_eq!(tx.execute(TRIM_TERMINALS,[0]).unwrap(),1);assert_eq!(count(&tx,"source_reset_facts"),0);assert_eq!(count(&tx,"source_reset_members"),0);assert_eq!(count(&tx,"audit"),0);assert_eq!(rows(&tx,"SELECT id FROM deliveries"),vec!["new"]);
  if commit{tx.commit().unwrap()}else{tx.rollback().unwrap();assert_eq!(count(&c,"deliveries"),2);assert_eq!(count(&c,"audit"),2);assert_eq!(c.query_row("SELECT refs FROM source_reset_facts",[],|r|r.get::<_,i64>(0)).unwrap(),3);assert_eq!(c.query_row("SELECT published FROM audit WHERE id='owed'",[],|r|r.get::<_,i64>(0)).unwrap(),0);}
 }
 assert!(rows(&c,"PRAGMA foreign_key_check").is_empty());
}
#[tokio::test]
async fn independent_one_retired_registration_does_not_block_other_lost_ack_recovery(){
 let mut f=Fixture::new().await;let mut second=f.registration.clone();second.id=Uuid::new_v4();second.target=Target{credential:CredentialId::new("synthetic-second-key").unwrap(),principal:PrincipalId::new("synthetic-second").unwrap(),organization:second.target.organization.clone(),owner:second.target.owner.clone()};
 f.store.install(second.clone(),1000).await.unwrap();
 f.store.queue(f.record(1)).await.unwrap();
 let mut other=f.record(1);other.registration=second.id;other.scope=Scope::new(Id::new(second.id.to_string()).unwrap(),other.scope.origin().clone(),other.scope.stream().clone(),other.scope.incarnation().clone(),other.scope.schema().clone(),other.scope.access_epoch().clone());let second_id=other.id;f.store.queue(other).await.unwrap();
 f.store.owner.connection.lock().unwrap().execute_batch("CREATE TRIGGER review_hold BEFORE UPDATE OF published ON audit WHEN json_extract(OLD.record,'$.cause')='drained' BEGIN SELECT RAISE(IGNORE); END;").unwrap();
 assert_eq!(f.store.stop(f.registration.target.credential.clone(),StopReason::Expired{observed_by:ExpiryObserver::Lifecycle},2000).await,Err(OutboxError::Conflict));
 {let c=f.store.owner.connection.lock().unwrap();assert_eq!(count(&c,"registrations"),1);assert_eq!(count(&c,"deliveries"),1);assert_eq!(count(&c,"retirements"),1);c.execute_batch("DROP TRIGGER review_hold").unwrap();}
 let reopened=reopen_store(&mut f).await.unwrap();let ready=reopened.due(3000).await.unwrap();assert_eq!(ready.len(),1);assert_eq!(ready[0].delivery.id(),second_id);assert_eq!(ready[0].registration.id,second.id);
 assert!(f.audits().iter().any(|a|a["cause"]=="drained"&&a["initiator"]=="lifecycle"));
}
