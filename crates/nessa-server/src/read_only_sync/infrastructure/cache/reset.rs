use super::{raw_records, rows, ReadOnlyCache};
use crate::read_only_sync::{
    application::{
        reset::CacheResets, CacheError, CachePolicy, CachedProgress, CatalogueResetReceipt,
        ResetReceipt,
    },
    domain::CacheReset,
};
use nessa_auth::application::ports::Clock;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use nessa_sdk::{
    application::agent_execution::sessions::CommittedTranscript,
    infrastructure::session_storage::TranscriptFold,
};

impl CacheResets for ReadOnlyCache {
    fn reset_records(&mut self, request: &CacheReset) -> Result<ResetReceipt, CacheError> {
        self.reset(request)
    }

    fn reset_catalogue(
        &mut self,
        request: &CacheReset,
    ) -> Result<CatalogueResetReceipt, CacheError> {
        ReadOnlyCache::reset_catalogue(self, request)
    }
}

pub(super) fn apply(
    connection: &mut Connection,
    policy: CachePolicy,
    clock: &dyn Clock,
    request: &CacheReset,
) -> Result<ResetReceipt, CacheError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(rows::database_error)?;
    if let Some(receipt) = receipt(&transaction, request)? {
        return Ok(receipt);
    }
    let scope = request.expected();
    if rows::fenced(&transaction, scope)? {
        return Err(CacheError::Fenced);
    }
    let before = rows::progress(&transaction, scope)?.ok_or(CacheError::Stale)?;
    if before.scope != *scope || before.generation != request.generation() {
        return Err(CacheError::Stale);
    }
    let fold = TranscriptFold::new(request.replacement().clone())
        .map_err(super::records::transcript_error)?;
    let checkpoint = fold
        .checkpoint_with_limit(policy.checkpoint_bytes())
        .map_err(super::records::transcript_error)?;
    let after = CachedProgress {
        scope: request.replacement().clone(),
        downloaded: 0,
        applied: 0,
        facts: 0,
        generation: before.generation.checked_add(1).ok_or(CacheError::Quota)?,
    };
    let observed_at_ms = clock.unix_milliseconds();
    raw_records::clear_transcript(&transaction, scope)?;
    rows::save_progress(&transaction, &after)?;
    rows::save_checkpoint(&transaction, &after.scope, &checkpoint, policy)?;
    save_receipt(&transaction, request, &before, &after, observed_at_ms)?;
    transaction.commit().map_err(|_| CacheError::Uncertain)?;
    Ok(ResetReceipt::new(
        request.clone(),
        before,
        after,
        observed_at_ms,
    ))
}

fn receipt(
    connection: &Connection,
    request: &CacheReset,
) -> Result<Option<ResetReceipt>, CacheError> {
    let old = request.expected();
    let new = request.replacement();
    // Exact retry correlation occurs in SQL. No external stored identity text is acquired.
    let saved = connection.query_row(
        "SELECT before_downloaded, before_applied, before_facts, after_generation, observed_at_ms
         FROM cache_resets WHERE operation=?1 AND caller=?2 AND receiver=?3 AND origin=?4 AND stream=?5
          AND old_incarnation=?6 AND old_schema=?7 AND old_epoch=?8
          AND new_incarnation=?9 AND new_schema=?10 AND new_epoch=?11 AND before_generation=?12
          AND cause='ExplicitReset' AND initiator='LocalOperator'",
        params![request.operation().as_str(), request.caller().as_str(), old.receiver().as_str(), old.origin().as_str(), old.stream().as_str(), old.incarnation().as_str(), old.schema().as_str(), old.access_epoch().as_str(), new.incarnation().as_str(), new.schema().as_str(), new.access_epoch().as_str(), request.generation().to_be_bytes().as_slice()],
        |row| Ok((|| {
            let before = CachedProgress { scope: old.clone(), downloaded: rows::number(row,0)?, applied: rows::number(row,1)?, facts: rows::number(row,2)?, generation: request.generation() };
            let after = CachedProgress { scope: new.clone(), downloaded:0, applied:0, facts:0, generation:rows::number(row,3)? };
            if before.applied > before.downloaded || after.generation != before.generation.checked_add(1).ok_or(CacheError::Corrupt)? {
                return Err(CacheError::Corrupt);
            }
            CommittedTranscript::validate_fact_count(before.applied, before.facts).map_err(|_| CacheError::Corrupt)?;
            Ok(ResetReceipt::new(request.clone(), before, after, rows::number(row,4)?))
        })()),
    ).optional().map_err(rows::database_error)?.transpose()?;
    if saved.is_some() {
        return Ok(saved);
    }
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM cache_resets WHERE operation=?1)",
            params![request.operation().as_str()],
            |row| row.get(0),
        )
        .map_err(rows::database_error)?;
    if exists {
        return Err(CacheError::ConflictingRecord);
    }
    Ok(None)
}

fn save_receipt(
    connection: &Connection,
    request: &CacheReset,
    before: &CachedProgress,
    after: &CachedProgress,
    observed_at_ms: u64,
) -> Result<(), CacheError> {
    let old = request.expected();
    let new = request.replacement();
    connection.execute(
        "INSERT INTO cache_resets (operation,caller,receiver,origin,stream,old_incarnation,old_schema,old_epoch,new_incarnation,new_schema,new_epoch,before_generation,before_downloaded,before_applied,before_facts,after_generation,observed_at_ms,cause,initiator)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,'ExplicitReset','LocalOperator')",
        params![request.operation().as_str(), request.caller().as_str(), old.receiver().as_str(), old.origin().as_str(), old.stream().as_str(), old.incarnation().as_str(), old.schema().as_str(), old.access_epoch().as_str(), new.incarnation().as_str(), new.schema().as_str(), new.access_epoch().as_str(), before.generation.to_be_bytes().as_slice(), before.downloaded.to_be_bytes().as_slice(), before.applied.to_be_bytes().as_slice(), before.facts.to_be_bytes().as_slice(), after.generation.to_be_bytes().as_slice(), observed_at_ms.to_be_bytes().as_slice()],
    ).map_err(rows::database_error)?;
    Ok(())
}
