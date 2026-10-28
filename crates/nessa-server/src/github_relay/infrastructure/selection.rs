//! One transaction changes the complete live-event selection, including
//! deletion evidence. A failed write leaves the previous selection intact.
use super::sqlite::{failure, SqliteRelayStore};
use crate::github_relay::{
    application::LiveEventStore,
    domain::{Binding, Failure},
};
use nessa_local_database::rusqlite::{params, OptionalExtension, TransactionBehavior};
#[async_trait::async_trait]
impl LiveEventStore for SqliteRelayStore {
    async fn select(
        &self,
        generation: u64,
        bindings: &[Binding],
        initiator: &str,
        accept_gaps: bool,
    ) -> Result<(), Failure> {
        if bindings.len() > crate::github_relay::domain::contract::BINDINGS as usize {
            return Err(Failure::Capacity);
        }
        let bindings = bindings.to_vec();
        let initiator = initiator.to_owned();
        let selection = self.selection.clone();
        self.run(move|db,now|{
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
            let existing=tx.prepare("SELECT user_id,repository_id,generation FROM bindings").map_err(failure)?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(failure)?.collect::<Result<Vec<_>,_>>().map_err(failure)?;
            for (user,repo,generation) in existing {
                if bindings.iter().any(|b|b.user_id()==user && b.repository_id()==repo) {continue;}
                tx.execute("INSERT INTO binding_audit(user_id,repository_id,before_generation,cause,initiator,observed_at) VALUES(?,?,?,'live_events_disabled',?,?)",params![user,repo,generation,initiator,now]).map_err(failure)?;
                tx.execute("INSERT INTO audit(user_id,repository_id,delivery_id,cause,initiator,observed_at) SELECT user_id,repository_id,delivery_id,'live_events_disabled',?,? FROM events WHERE user_id=? AND repository_id=?",params![initiator,now,user,repo]).map_err(failure)?;
                tx.execute("DELETE FROM events WHERE user_id=? AND repository_id=?",params![user,repo]).map_err(failure)?;
                tx.execute("DELETE FROM bindings WHERE user_id=? AND repository_id=?",params![user,repo]).map_err(failure)?;
            }
            for b in bindings {
                let previous:Option<String>=tx.query_row("SELECT generation FROM bindings WHERE user_id=? AND repository_id=?",params![b.user_id(),b.repository_id()],|r|r.get(0)).optional().map_err(failure)?;
                if previous.is_some() {
                    tx.execute("UPDATE bindings SET repository_name=? WHERE user_id=? AND repository_id=?",params![b.repository_name(),b.user_id(),b.repository_id()]).map_err(failure)?;
                    if accept_gaps {
                        tx.execute("INSERT INTO binding_audit(user_id,repository_id,before_generation,after_generation,before_cursor,after_cursor,cause,initiator,observed_at) SELECT user_id,repository_id,generation,?,cursor,gap_through,'gap_accepted',?,? FROM bindings WHERE user_id=? AND repository_id=? AND gap_through IS NOT NULL",params![b.generation().to_string(),initiator,now,b.user_id(),b.repository_id()]).map_err(failure)?;
                        tx.execute("UPDATE bindings SET cursor=MAX(cursor,gap_through),gap_through=NULL,generation=? WHERE user_id=? AND repository_id=? AND gap_through IS NOT NULL",params![b.generation().to_string(),b.user_id(),b.repository_id()]).map_err(failure)?;
                    }
                    continue;
                }
                tx.execute("INSERT INTO binding_audit(user_id,repository_id,after_generation,cause,initiator,observed_at) VALUES(?,?,?,'live_events_enabled',?,?)",params![b.user_id(),b.repository_id(),b.generation().to_string(),initiator,now]).map_err(failure)?;
                tx.execute("INSERT INTO bindings(user_id,repository_id,repository_name,generation) VALUES(?,?,?,?)",params![b.user_id(),b.repository_id(),b.repository_name(),b.generation().to_string()]).map_err(failure)?;
            }
            for table in ["audit","binding_audit"] {tx.execute(&format!("DELETE FROM {table} WHERE id IN (SELECT id FROM {table} ORDER BY id DESC LIMIT -1 OFFSET ?)"),[crate::github_relay::domain::contract::AUDIT_ROWS]).map_err(failure)?;}
            let mut pending = Some(tx);
            selection.commit_in_generation(generation, &mut || {
                pending
                    .take()
                    .expect("selection commits once")
                    .commit()
                    .map_err(failure)
            })?;
            if pending.is_some() {
                return Err(Failure::Storage);
            }
            Ok(())
        }).await
    }
}
