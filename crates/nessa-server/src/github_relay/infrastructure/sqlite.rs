use crate::github_relay::domain::contract;
use crate::github_relay::{
    application::{InboxKeySource, LiveSelectionAdmission, RelayStore},
    domain::{Binding, Event, Failure, Page},
};
use nessa_local_database::{
    open_encrypted,
    rusqlite::{params, OptionalExtension},
    Schema,
};
use std::{path::PathBuf, sync::Arc};

pub struct SqliteRelayStore {
    path: PathBuf,
    key: Arc<dyn InboxKeySource>,
    pub(super) selection: Arc<dyn LiveSelectionAdmission>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}
impl SqliteRelayStore {
    pub fn new(
        path: PathBuf,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        key: Arc<dyn InboxKeySource>,
        selection: Arc<dyn LiveSelectionAdmission>,
    ) -> Self {
        Self {
            path,
            now,
            key,
            selection,
        }
    }
    pub(super) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut nessa_local_database::rusqlite::Connection, i64) -> Result<T, Failure>
            + Send
            + 'static,
    ) -> Result<T, Failure> {
        let path = self.path.clone();
        let now = (self.now)();
        let key = self.key.clone();
        tokio::task::spawn_blocking(move || {
            let parent = path.parent().ok_or(Failure::Storage)?;
            nessa_local_storage::create_directory(parent).map_err(|_| Failure::Storage)?;
            let schema = Schema::new(include_str!("schema.sql")).map_err(|_| Failure::Storage)?;
            let key = key.load()?;
            let mut connection =
                open_encrypted(&path, &schema, &key).map_err(|_| Failure::Storage)?;
            operation(&mut connection, now)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
}
pub(super) fn failure(_: nessa_local_database::rusqlite::Error) -> Failure {
    Failure::Storage
}
/// Drops the oldest rows down to the contract's `auditRows`. Call it in the
/// transaction that inserted, before commit. The new row has the greatest id,
/// so `direct_gap_and_acceptance_keep_each_audit_store_within_its_cap` still
/// reads that transition's target, cause, actor, and cursors.
/// `trim_binding_audit` is the same rule for generation evidence.
pub(super) fn trim_event_audit(
    tx: &nessa_local_database::rusqlite::Transaction,
) -> Result<(), Failure> {
    tx.execute(
        "DELETE FROM audit WHERE id IN (SELECT id FROM audit ORDER BY id DESC LIMIT -1 OFFSET ?)",
        [contract::AUDIT_ROWS],
    )
    .map_err(failure)?;
    Ok(())
}
pub(super) fn trim_binding_audit(
    tx: &nessa_local_database::rusqlite::Transaction,
) -> Result<(), Failure> {
    tx.execute(
        "DELETE FROM binding_audit WHERE id IN (SELECT id FROM binding_audit ORDER BY id DESC LIMIT -1 OFFSET ?)",
        [contract::AUDIT_ROWS],
    )
    .map_err(failure)?;
    Ok(())
}
#[async_trait::async_trait]
impl RelayStore for SqliteRelayStore {
    async fn bindings(&self) -> Result<Vec<Binding>, Failure> {
        self.run(|db, _| {
            db.prepare("SELECT user_id, repository_id, repository_name, generation FROM bindings ORDER BY user_id, repository_id").map_err(failure)?
                .query_map([], |row| Binding::new(row.get(0)?, row.get(1)?, row.get(2)?, uuid::Uuid::parse_str(&row.get::<_,String>(3)?).map_err(|_| nessa_local_database::rusqlite::Error::InvalidQuery)?).map_err(|_| nessa_local_database::rusqlite::Error::InvalidQuery)).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)
        }).await
    }
    async fn bind(&self, binding: &Binding, accept_gap: bool) -> Result<(), Failure> {
        let binding = binding.clone();
        self.run(move |db, now| {
            let tx = db.transaction_with_behavior(nessa_local_database::rusqlite::TransactionBehavior::Immediate).map_err(failure)?;
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM bindings", [], |row| row.get(0)).map_err(failure)?;
            let existing: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM bindings WHERE user_id=? AND repository_id=?)", params![binding.user_id(),binding.repository_id()], |row| row.get(0)).map_err(failure)?;
            if !existing && count >= contract::BINDINGS { return Err(Failure::Capacity); }
            let previous: Option<String> = tx.query_row("SELECT generation FROM bindings WHERE user_id=? AND repository_id=?",params![binding.user_id(),binding.repository_id()],|row|row.get(0)).optional().map_err(failure)?;
            let generation=binding.generation().to_string();
            if previous.as_deref()!=Some(&generation) {
                tx.execute("INSERT INTO binding_audit(user_id,repository_id,before_generation,after_generation,cause,initiator,observed_at) VALUES(?,?,?,?, 'connected','local-owner',?)",params![binding.user_id(),binding.repository_id(),previous,generation,now]).map_err(failure)?;
            }
            tx.execute("INSERT INTO bindings(user_id,repository_id,repository_name,generation) VALUES(?,?,?,?) ON CONFLICT(user_id,repository_id) DO UPDATE SET repository_name=excluded.repository_name,generation=excluded.generation", params![binding.user_id(),binding.repository_id(),binding.repository_name(),generation]).map_err(failure)?;
            if accept_gap {
                tx.execute("INSERT INTO audit(user_id,repository_id,before_cursor,after_cursor,cause,initiator,observed_at) SELECT user_id,repository_id,cursor,gap_through,'gap_accepted',?,? FROM bindings WHERE user_id=? AND repository_id=? AND gap_through IS NOT NULL",params![format!("local-owner:github:{}",binding.user_id()),now,binding.user_id(),binding.repository_id()]).map_err(failure)?;
                tx.execute("UPDATE bindings SET cursor=MAX(cursor,COALESCE(gap_through,cursor)), gap_through=NULL WHERE user_id=? AND repository_id=?",params![binding.user_id(),binding.repository_id()]).map_err(failure)?;
            }
            if !existing { tx.execute("INSERT INTO audit(user_id,repository_id,cause,initiator,observed_at) VALUES(?,?,'connected',?,?)",params![binding.user_id(),binding.repository_id(),format!("local-owner:github:{}",binding.user_id()),now]).map_err(failure)?; }
            trim_event_audit(&tx)?;
            trim_binding_audit(&tx)?;
            tx.commit().map_err(failure)
        }).await
    }
    async fn unbind(&self, binding: &Binding) -> Result<(), Failure> {
        let binding = binding.clone();
        self.run(move |db,now| {
            let tx=db.transaction_with_behavior(nessa_local_database::rusqlite::TransactionBehavior::Immediate).map_err(failure)?;
            let current: Option<String> = tx.query_row("SELECT generation FROM bindings WHERE user_id=? AND repository_id=?",params![binding.user_id(),binding.repository_id()],|row|row.get(0)).optional().map_err(failure)?;
            if current.as_deref()!=Some(&binding.generation().to_string()) { return Err(Failure::CursorChanged); }
            tx.execute("INSERT INTO binding_audit(user_id,repository_id,before_generation,before_cursor,cause,initiator,observed_at) SELECT user_id,repository_id,generation,cursor,'disconnected','local-owner',? FROM bindings WHERE user_id=? AND repository_id=?",params![now,binding.user_id(),binding.repository_id()]).map_err(failure)?;
            tx.execute("INSERT INTO audit(user_id,repository_id,delivery_id,cause,initiator,observed_at) SELECT user_id,repository_id,delivery_id,'disconnected','local-owner',? FROM events WHERE user_id=? AND repository_id=?",params![now,binding.user_id(),binding.repository_id()]).map_err(failure)?;
            tx.execute("DELETE FROM events WHERE user_id=? AND repository_id=?",params![binding.user_id(),binding.repository_id()]).map_err(failure)?;
            tx.execute("DELETE FROM bindings WHERE user_id=? AND repository_id=?",params![binding.user_id(),binding.repository_id()]).map_err(failure)?;
            trim_event_audit(&tx)?;
            trim_binding_audit(&tx)?;
            tx.commit().map_err(failure)
        }).await
    }
    async fn cursor(&self, binding: &Binding) -> Result<i64, Failure> {
        let binding = binding.clone();
        self.run(move |db, _| {
            let saved: Option<(i64, Option<i64>)> = db
                .query_row(
                    "SELECT cursor,gap_through FROM bindings WHERE user_id=? AND repository_id=? AND generation=?",
                    params![binding.user_id(), binding.repository_id(),binding.generation().to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional().map_err(failure)?;
            let (cursor, gap) = saved.ok_or(Failure::CursorChanged)?;
            if let Some(through) = gap {
                return Err(Failure::Gap(through));
            }
            Ok(cursor)
        })
        .await
    }
    async fn commit(&self, binding: &Binding, after: i64, page: &Page) -> Result<(), Failure> {
        page.validate(binding, after)?;
        let binding = binding.clone();
        let events = page.events.clone();
        let next = page.next;
        self.run(move |db,now| {
            let tx=db.transaction_with_behavior(nessa_local_database::rusqlite::TransactionBehavior::Immediate).map_err(failure)?;
            let saved: Option<(i64,Option<i64>)> = tx.query_row("SELECT cursor,gap_through FROM bindings WHERE user_id=? AND repository_id=? AND generation=?",params![binding.user_id(),binding.repository_id(),binding.generation().to_string()],|row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(failure)?;
            if saved != Some((after,None)) { return Err(Failure::CursorChanged); }
            let before=now.saturating_sub(contract::RETENTION_SECONDS);
            tx.execute("INSERT INTO audit(user_id,repository_id,delivery_id,cause,initiator,observed_at) SELECT user_id,repository_id,delivery_id,'retention_expired','system:local-relay',? FROM events WHERE stored_at < ?",params![now,before]).map_err(failure)?;
            tx.execute("DELETE FROM events WHERE stored_at < ?",[before]).map_err(failure)?;
            let mut unseen=Vec::new();
            for event in &events {
                let prior:Option<(String,String)>=tx.query_row("SELECT event,payload FROM events WHERE user_id=? AND repository_id=? AND delivery_id=?",params![binding.user_id(),binding.repository_id(),event.delivery_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(failure)?;
                match prior {
                    Some((name,payload)) if name==event.event && payload==event.payload=>(),
                    Some(_)=>return Err(Failure::InvalidResponse),
                    None=>unseen.push(event),
                }
            }
            let (count,bytes):(i64,i64)=tx.query_row("SELECT COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events",[],|row| Ok((row.get(0)?,row.get(1)?))).map_err(failure)?;
            if count+unseen.len() as i64>contract::STORED_EVENTS || bytes+unseen.iter().map(|e|e.payload.len() as i64).sum::<i64>()>contract::STORED_BYTES { return Err(Failure::Capacity); }
            for event in &unseen {
                tx.execute("INSERT INTO events(user_id,repository_id,sequence,delivery_id,event,payload,received_at,stored_at) VALUES(?,?,?,?,?,?,?,?)",params![binding.user_id(),binding.repository_id(),event.sequence,event.delivery_id,event.event,event.payload,event.received_at,now]).map_err(failure)?;
            }
            tx.execute("UPDATE bindings SET cursor=? WHERE user_id=? AND repository_id=?",params![next,binding.user_id(),binding.repository_id()]).map_err(failure)?;
            if next!=after { tx.execute("INSERT INTO binding_audit(user_id,repository_id,before_generation,after_generation,before_cursor,after_cursor,cause,initiator,observed_at) VALUES(?,?,?,?,?,?,'page_committed','system:local-relay',?)",params![binding.user_id(),binding.repository_id(),binding.generation().to_string(),binding.generation().to_string(),after,next,now]).map_err(failure)?; }
            trim_event_audit(&tx)?;
            trim_binding_audit(&tx)?;
            tx.commit().map_err(failure)
        }).await
    }
    async fn gap(&self, binding: &Binding, through: i64) -> Result<(), Failure> {
        let binding = binding.clone();
        self.run(move |db,now| {
            let tx=db.transaction_with_behavior(nessa_local_database::rusqlite::TransactionBehavior::Immediate).map_err(failure)?;
            let current: Option<String> = tx.query_row("SELECT generation FROM bindings WHERE user_id=? AND repository_id=?",params![binding.user_id(),binding.repository_id()],|row|row.get(0)).optional().map_err(failure)?;
            if current.as_deref()!=Some(&binding.generation().to_string()) { return Err(Failure::CursorChanged); }
            tx.execute("INSERT INTO audit(user_id,repository_id,before_cursor,after_cursor,cause,initiator,observed_at) SELECT user_id,repository_id,cursor,?,'cloud_retention_gap','system:cloud-relay',? FROM bindings WHERE user_id=? AND repository_id=? AND gap_through IS NULL",params![through,now,binding.user_id(),binding.repository_id()]).map_err(failure)?;
            tx.execute("UPDATE bindings SET gap_through=MAX(COALESCE(gap_through,0),?) WHERE user_id=? AND repository_id=?",params![through,binding.user_id(),binding.repository_id()]).map_err(failure)?;
            trim_event_audit(&tx)?;
            tx.commit().map_err(failure)
        }).await
    }
    async fn events(&self, binding: &Binding) -> Result<Vec<Event>, Failure> {
        let binding = binding.clone();
        self.run(move |db,_| db.prepare("SELECT sequence,delivery_id,event,payload,received_at,repository_id FROM events WHERE user_id=? AND repository_id=? ORDER BY sequence DESC LIMIT 100").map_err(failure)?.query_map(params![binding.user_id(),binding.repository_id()],|row| Ok(Event {sequence:row.get(0)?,delivery_id:row.get(1)?,event:row.get(2)?,payload:row.get(3)?,received_at:row.get(4)?,repository_id:row.get(5)?})).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)).await
    }
}
