//! The follower's grant replica. A complete frame is applied in one write
//! with the cursor it names. An incomplete frame is not stored here.

use super::records::ReadOnlyCache;
use crate::read_only_sync::application::CacheError;
use nessa_local_database::rusqlite::params;

impl ReadOnlyCache {
    pub(crate) fn grant_cursor(&self, receiver: &str) -> Result<Option<(String, u64)>, CacheError> {
        let found = self
            .connection
            .query_row(
                "SELECT incarnation, position FROM grant_cursors WHERE receiver = ?1",
                params![receiver],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(super::rows::database_error)?;
        found
            .map(|(incarnation, position)| {
                u64::try_from(position)
                    .map(|position| (incarnation, position))
                    .map_err(|_| CacheError::Corrupt)
            })
            .transpose()
    }

    pub(crate) fn grant_ids(&self, receiver: &str) -> Result<Vec<String>, CacheError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT conversation_id FROM grant_rows WHERE receiver = ?1 ORDER BY conversation_id",
            )
            .map_err(super::rows::database_error)?;
        let rows = statement
            .query_map(params![receiver], |row| row.get(0))
            .map_err(super::rows::database_error)?;
        rows.collect::<Result<Vec<String>, _>>()
            .map_err(super::rows::database_error)
    }

    /// The grants this receiver still holds, in conversation id order.
    pub(crate) fn grant_present(
        &self,
        receiver: &str,
    ) -> Result<Vec<(String, String, i64)>, CacheError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT conversation_id, capabilities, policy_revision
                 FROM grant_rows WHERE receiver = ?1 ORDER BY conversation_id",
            )
            .map_err(super::rows::database_error)?;
        let rows = statement
            .query_map(params![receiver], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(super::rows::database_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(super::rows::database_error)
    }

    /// Remove the grants whose last record in `entries` is a tombstone, and,
    /// when `replaces`, every stored grant the frame no longer names. The
    /// cursor stays where it was, so a later copy of what remains can still
    /// fail and be asked again. Returns the ids that left the replica.
    pub(crate) fn withdraw_grant_entries(
        &mut self,
        receiver: &str,
        replaces: bool,
        entries: &[(String, bool, Option<String>, i64)],
    ) -> Result<Vec<String>, CacheError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(super::rows::database_error)?;
        let withdrawn = withdraw_entries(&transaction, receiver, replaces, entries)?;
        transaction.commit().map_err(super::rows::database_error)?;
        self.invalidate_loaded();
        Ok(withdrawn)
    }

    /// Store the grants that remain and the cursor that names this complete
    /// frame. Call it only after those conversations' records have been copied.
    pub(crate) fn commit_grant_entries(
        &mut self,
        receiver: &str,
        incarnation: &str,
        position: u64,
        entries: &[(String, bool, Option<String>, i64)],
    ) -> Result<(), CacheError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(super::rows::database_error)?;
        for (conversation, capabilities, revision) in last_present(entries) {
            upsert_grant(
                &transaction,
                receiver,
                &conversation,
                &capabilities,
                revision,
            )?;
        }
        store_cursor(&transaction, receiver, incarnation, position)?;
        transaction.commit().map_err(super::rows::database_error)?;
        self.invalidate_loaded();
        Ok(())
    }

    /// Apply `entries` in commit order and store the cursor. A later record for
    /// the same conversation wins. Removing a grant also removes that
    /// conversation's retained transcript. Returns the ids that left the replica.
    #[cfg(test)]
    pub(crate) fn apply_grant_entries(
        &mut self,
        receiver: &str,
        replaces: bool,
        incarnation: &str,
        position: u64,
        entries: &[(String, bool, Option<String>, i64)],
    ) -> Result<Vec<String>, CacheError> {
        let transaction = self
            .connection
            .transaction()
            .map_err(super::rows::database_error)?;
        let withdrawn = withdraw_entries(&transaction, receiver, replaces, entries)?;
        for (conversation, capabilities, revision) in last_present(entries) {
            upsert_grant(
                &transaction,
                receiver,
                &conversation,
                &capabilities,
                revision,
            )?;
        }
        store_cursor(&transaction, receiver, incarnation, position)?;
        transaction.commit().map_err(super::rows::database_error)?;
        self.invalidate_loaded();
        Ok(withdrawn)
    }

    pub(crate) fn clear_grant_cursor(&mut self, receiver: &str) -> Result<(), CacheError> {
        self.connection
            .execute(
                "DELETE FROM grant_cursors WHERE receiver = ?1",
                params![receiver],
            )
            .map_err(super::rows::database_error)?;
        Ok(())
    }
}

use nessa_local_database::rusqlite::{OptionalExtension, Transaction};

/// Last record that still grants each conversation. An earlier tombstone does
/// not survive a later grant in the same frame.
fn last_present(entries: &[(String, bool, Option<String>, i64)]) -> Vec<(String, String, i64)> {
    let mut last: Vec<&(String, bool, Option<String>, i64)> = Vec::new();
    for entry in entries {
        last.retain(|kept| kept.0 != entry.0);
        last.push(entry);
    }
    last.into_iter()
        .filter_map(|(conversation, absent, capabilities, revision)| {
            if *absent {
                return None;
            }
            Some((conversation.clone(), capabilities.clone()?, *revision))
        })
        .collect()
}

fn withdraw_entries(
    transaction: &Transaction<'_>,
    receiver: &str,
    replaces: bool,
    entries: &[(String, bool, Option<String>, i64)],
) -> Result<Vec<String>, CacheError> {
    let present = last_present(entries);
    let mut withdrawn = Vec::new();
    if replaces {
        let mut statement = transaction
            .prepare("SELECT conversation_id FROM grant_rows WHERE receiver = ?1")
            .map_err(super::rows::database_error)?;
        let existing: Vec<String> = statement
            .query_map(params![receiver], |row| row.get(0))
            .map_err(super::rows::database_error)?
            .collect::<Result<_, _>>()
            .map_err(super::rows::database_error)?;
        drop(statement);
        for id in existing {
            if !present.iter().any(|(kept, _, _)| kept == &id) {
                remove_grant_conversation(transaction, receiver, &id)?;
                withdrawn.push(id);
            }
        }
        // A copy can store a transcript and then fail before the grant row
        // is committed. A later replacement that no longer names that chat
        // has no grant row to find, so the transcript is listed on its own.
        let mut statement = transaction
            .prepare("SELECT DISTINCT stream FROM transcript_progress WHERE receiver = ?1")
            .map_err(super::rows::database_error)?;
        let copied: Vec<String> = statement
            .query_map(params![receiver], |row| row.get(0))
            .map_err(super::rows::database_error)?
            .collect::<Result<_, _>>()
            .map_err(super::rows::database_error)?;
        drop(statement);
        for id in copied {
            if present.iter().any(|(kept, _, _)| kept == &id)
                || withdrawn.iter().any(|gone| gone == &id)
            {
                continue;
            }
            remove_grant_conversation(transaction, receiver, &id)?;
            withdrawn.push(id);
        }
        return Ok(withdrawn);
    }
    let mut last: Vec<&(String, bool, Option<String>, i64)> = Vec::new();
    for entry in entries {
        last.retain(|kept| kept.0 != entry.0);
        last.push(entry);
    }
    for (conversation, absent, capabilities, _) in last {
        if (*absent || capabilities.is_none())
            && remove_grant_conversation(transaction, receiver, conversation)?
        {
            withdrawn.push(conversation.clone());
        }
    }
    Ok(withdrawn)
}

fn store_cursor(
    transaction: &Transaction<'_>,
    receiver: &str,
    incarnation: &str,
    position: u64,
) -> Result<(), CacheError> {
    let position = i64::try_from(position).map_err(|_| CacheError::Corrupt)?;
    transaction
        .execute(
            "INSERT INTO grant_cursors (receiver, incarnation, position) VALUES (?1, ?2, ?3)
             ON CONFLICT(receiver) DO UPDATE SET incarnation = excluded.incarnation, position = excluded.position",
            params![receiver, incarnation, position],
        )
        .map_err(super::rows::database_error)?;
    Ok(())
}

fn upsert_grant(
    transaction: &Transaction<'_>,
    receiver: &str,
    conversation: &str,
    capabilities: &str,
    revision: i64,
) -> Result<(), CacheError> {
    transaction
        .execute(
            "INSERT INTO grant_rows (receiver, conversation_id, capabilities, policy_revision)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(receiver, conversation_id) DO UPDATE SET
               capabilities = excluded.capabilities,
               policy_revision = excluded.policy_revision",
            params![receiver, conversation, capabilities, revision],
        )
        .map_err(super::rows::database_error)?;
    Ok(())
}

/// Drop the grant row and the transcript stored for that conversation.
/// True when a grant row was removed.
fn remove_grant_conversation(
    transaction: &Transaction<'_>,
    receiver: &str,
    conversation: &str,
) -> Result<bool, CacheError> {
    let removed = transaction
        .execute(
            "DELETE FROM grant_rows WHERE receiver = ?1 AND conversation_id = ?2",
            params![receiver, conversation],
        )
        .map_err(super::rows::database_error)?;
    for table in [
        "transcript_records",
        "transcript_checkpoints",
        "transcript_progress",
        "catalogue_entries",
    ] {
        let column = if table == "catalogue_entries" {
            "entry_id"
        } else {
            "stream"
        };
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE receiver = ?1 AND {column} = ?2"),
                params![receiver, conversation],
            )
            .map_err(super::rows::database_error)?;
    }
    Ok(removed > 0)
}
