//! Attributed host reset; the generic core port cannot supply local operator evidence.
use super::{catalogue_rows, rows, ReadOnlyCache};
use crate::read_only_sync::{
    application::{CacheError, CatalogueResetReceipt},
    domain::CacheReset,
};
use nessa_local_database::rusqlite::{
    params, types::ValueRef, Connection, OptionalExtension, TransactionBehavior,
};
use nessa_sync::replication::catalogue::{
    catalogue_progress_after_reset, CataloguePass, CatalogueProgress, EntryKey,
};

impl ReadOnlyCache {
    pub(crate) fn reset_catalogue(
        &mut self,
        request: &CacheReset,
    ) -> Result<CatalogueResetReceipt, CacheError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        if let Some(receipt) = receipt(&tx, request)? {
            return Ok(receipt);
        }
        let before = catalogue_rows::progress(&tx, request.expected())?.ok_or(CacheError::Stale)?;
        if before.scope != *request.expected() || before.generation != request.generation() {
            return Err(CacheError::Stale);
        }
        let after = catalogue_progress_after_reset(request.replacement(), &before)
            .map_err(CacheError::CatalogueProgress)?;
        tx.execute("DELETE FROM catalogue_entries WHERE receiver=?1 AND origin=?2 AND stream=?3 AND deleted=0",params![before.scope.receiver().as_str(),before.scope.origin().as_str(),before.scope.stream().as_str()]).map_err(rows::database_error)?;
        catalogue_rows::save_progress(&tx, &after)?;
        let observed = self.clock.unix_milliseconds();
        save_receipt(&tx, request, &before, &after, observed)?;
        tx.commit().map_err(|_| CacheError::Uncertain)?;
        Ok(CatalogueResetReceipt::new(
            request.clone(),
            before,
            after,
            observed,
        ))
    }
}

fn receipt(
    connection: &Connection,
    request: &CacheReset,
) -> Result<Option<CatalogueResetReceipt>, CacheError> {
    let old = request.expected();
    let new = request.replacement();
    let result=connection.query_row(
        "SELECT before_completed,before_boundary,before_cursor_creation,
                CASE WHEN octet_length(before_cursor_id)<=?13 THEN before_cursor_id END,
                before_cursor_id IS NOT NULL,after_generation,observed_at_ms
         FROM catalogue_resets WHERE operation=?1 AND caller=?2 AND receiver=?3 AND origin=?4 AND stream=?5
         AND old_incarnation=?6 AND old_schema=?7 AND old_epoch=?8
         AND new_incarnation=?9 AND new_schema=?10 AND new_epoch=?11 AND before_generation=?12
         AND cause='ExplicitReset' AND initiator='LocalOperator'",
        params![request.operation().as_str(),request.caller().as_str(),old.receiver().as_str(),old.origin().as_str(),old.stream().as_str(),old.incarnation().as_str(),old.schema().as_str(),old.access_epoch().as_str(),new.incarnation().as_str(),new.schema().as_str(),new.access_epoch().as_str(),request.generation().to_be_bytes().as_slice(),rows::MAX_STORED_ID_BYTES as i64],
        |row|Ok((||{
            let completed=rows::number(row,0)?;
            let boundary=if matches!(row.get_ref(1).map_err(rows::database_error)?,ValueRef::Null){None}else{Some(rows::number(row,1)?)};
            let creation=if matches!(row.get_ref(2).map_err(rows::database_error)?,ValueRef::Null){None}else{Some(rows::number(row,2)?)};
            let present:bool=row.get(4).map_err(rows::database_error)?;
            let cursor=match (creation,present){(None,false)=>None,(Some(creation),true)=>Some(EntryKey{creation,id:rows::identifier(row,3)?}),_=>return Err(CacheError::Corrupt)};
            if boundary.is_none() && cursor.is_some(){return Err(CacheError::Corrupt);}
            let before=CatalogueProgress{scope:old.clone(),completed,generation:request.generation(),active:boundary.map(|boundary|CataloguePass{scope:old.clone(),completed,boundary,cursor,generation:request.generation()})};
            let after=catalogue_progress_after_reset(new,&before).map_err(CacheError::CatalogueProgress)?;
            if rows::number(row,5)?!=after.generation{return Err(CacheError::Corrupt);}
            Ok(CatalogueResetReceipt::new(request.clone(),before,after,rows::number(row,6)?))
        })()),
    ).optional().map_err(rows::database_error)?.transpose()?;
    if result.is_some() {
        return Ok(result);
    }
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM catalogue_resets WHERE operation=?1)",
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
    before: &CatalogueProgress,
    after: &CatalogueProgress,
    observed: u64,
) -> Result<(), CacheError> {
    let old = request.expected();
    let new = request.replacement();
    let boundary = before
        .active
        .as_ref()
        .map(|pass| pass.boundary.to_be_bytes());
    let cursor = before.active.as_ref().and_then(|pass| pass.cursor.as_ref());
    let creation = cursor.map(|key| key.creation.to_be_bytes());
    connection.execute(
        "INSERT INTO catalogue_resets(operation,caller,receiver,origin,stream,old_incarnation,old_schema,old_epoch,new_incarnation,new_schema,new_epoch,before_generation,before_completed,before_boundary,before_cursor_creation,before_cursor_id,after_generation,observed_at_ms,cause,initiator)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,'ExplicitReset','LocalOperator')",
        params![request.operation().as_str(),request.caller().as_str(),old.receiver().as_str(),old.origin().as_str(),old.stream().as_str(),old.incarnation().as_str(),old.schema().as_str(),old.access_epoch().as_str(),new.incarnation().as_str(),new.schema().as_str(),new.access_epoch().as_str(),before.generation.to_be_bytes().as_slice(),before.completed.to_be_bytes().as_slice(),boundary.as_ref().map(|v|v.as_slice()),creation.as_ref().map(|v|v.as_slice()),cursor.map(|key|key.id.as_str()),after.generation.to_be_bytes().as_slice(),observed.to_be_bytes().as_slice()],
    ).map_err(rows::database_error)?;
    Ok(())
}
