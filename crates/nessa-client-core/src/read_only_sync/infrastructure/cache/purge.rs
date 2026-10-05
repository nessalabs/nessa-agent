//! Deleting one receiver's cached data after the gateway's authenticated
//! Terminal status, with its receipt in the same transaction. The receipt is
//! the fence later applies consult (`rows::purged`). The cause is the Terminal
//! enrollment status itself; the gateway keeps why the enrollment ended.
use super::{rows, ReadOnlyCache};
use crate::read_only_sync::application::{
    device::{CachePurges, PurgeReceipt},
    CacheError,
};
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use nessa_sync::replication::domain::Id;

impl CachePurges for ReadOnlyCache {
    fn purge_receiver(&mut self, receiver: &Id) -> Result<PurgeReceipt, CacheError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(rows::database_error)?;
        if let Some(saved) = receipt(&transaction, receiver)? {
            return Ok(saved);
        }
        let count = |sql: &str| -> Result<u64, CacheError> {
            transaction
                .query_row(sql, params![receiver.as_str()], |row| row.get::<_, i64>(0))
                .map_err(rows::database_error)
                .and_then(|value| u64::try_from(value).map_err(|_| CacheError::Corrupt))
        };
        let purged = PurgeReceipt {
            receiver: receiver.clone(),
            transcripts: count("SELECT count(*) FROM transcript_progress WHERE receiver=?1")?,
            records: count("SELECT count(*) FROM transcript_records WHERE receiver=?1")?,
            catalogue_entries: count("SELECT count(*) FROM catalogue_entries WHERE receiver=?1")?,
            observed_at_ms: self.clock.unix_milliseconds(),
        };
        // Children before the progress rows their foreign keys name. Deletion
        // fences and reset receipts are evidence and stay.
        for table in [
            "transcript_records",
            "transcript_checkpoints",
            "transcript_progress",
            "catalogue_entries",
            "catalogue_progress",
        ] {
            transaction
                .execute(
                    &format!("DELETE FROM {table} WHERE receiver=?1"),
                    params![receiver.as_str()],
                )
                .map_err(rows::database_error)?;
        }
        transaction
            .execute(
                "INSERT INTO cache_purges (receiver,cause,initiator,transcripts,records,catalogue_entries,observed_at_ms)
                 VALUES (?1,'TerminalEnrollment','GatewayStatus',?2,?3,?4,?5)",
                params![
                    receiver.as_str(),
                    purged.transcripts.to_be_bytes().as_slice(),
                    purged.records.to_be_bytes().as_slice(),
                    purged.catalogue_entries.to_be_bytes().as_slice(),
                    purged.observed_at_ms.to_be_bytes().as_slice(),
                ],
            )
            .map_err(rows::database_error)?;
        let committed = transaction.commit();
        // Whatever the outcome, no loaded fold may outlive rows it came from.
        self.invalidate_loaded();
        committed.map_err(|_| CacheError::Uncertain)?;
        Ok(purged)
    }
}

fn receipt(connection: &Connection, receiver: &Id) -> Result<Option<PurgeReceipt>, CacheError> {
    connection
        .query_row(
            "SELECT transcripts, records, catalogue_entries, observed_at_ms
             FROM cache_purges WHERE receiver=?1
              AND cause='TerminalEnrollment' AND initiator='GatewayStatus'",
            params![receiver.as_str()],
            |row| {
                Ok((|| {
                    Ok(PurgeReceipt {
                        receiver: receiver.clone(),
                        transcripts: rows::number(row, 0)?,
                        records: rows::number(row, 1)?,
                        catalogue_entries: rows::number(row, 2)?,
                        observed_at_ms: rows::number(row, 3)?,
                    })
                })())
            },
        )
        .optional()
        .map_err(rows::database_error)?
        .transpose()
}
