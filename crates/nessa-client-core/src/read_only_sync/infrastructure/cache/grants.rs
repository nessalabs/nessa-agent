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

    /// Replace or continue this receiver's replica and store `cursor` when
    /// `complete`. Returns the conversation ids that were held and are not
    /// in the resulting set.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_grant_frame(
        &mut self,
        receiver: &str,
        replaces: bool,
        complete: bool,
        incarnation: &str,
        position: u64,
        present: &[(String, String, i64)],
        absent: &[String],
    ) -> Result<Vec<String>, CacheError> {
        if !complete {
            return Ok(Vec::new());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(super::rows::database_error)?;
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
            transaction
                .execute(
                    "DELETE FROM grant_rows WHERE receiver = ?1",
                    params![receiver],
                )
                .map_err(super::rows::database_error)?;
            for (conversation, capabilities, revision) in present {
                transaction
                    .execute(
                        "INSERT INTO grant_rows (receiver, conversation_id, capabilities, policy_revision)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![receiver, conversation, capabilities, revision],
                    )
                    .map_err(super::rows::database_error)?;
            }
            for id in existing {
                if !present.iter().any(|(kept, _, _)| kept == &id) {
                    withdrawn.push(id);
                }
            }
        } else {
            for id in absent {
                let removed = transaction
                    .execute(
                        "DELETE FROM grant_rows WHERE receiver = ?1 AND conversation_id = ?2",
                        params![receiver, id],
                    )
                    .map_err(super::rows::database_error)?;
                if removed > 0 {
                    withdrawn.push(id.clone());
                }
            }
            for (conversation, capabilities, revision) in present {
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
            }
        }
        let position = i64::try_from(position).map_err(|_| CacheError::Corrupt)?;
        transaction
            .execute(
                "INSERT INTO grant_cursors (receiver, incarnation, position) VALUES (?1, ?2, ?3)
                 ON CONFLICT(receiver) DO UPDATE SET incarnation = excluded.incarnation, position = excluded.position",
                params![receiver, incarnation, position],
            )
            .map_err(super::rows::database_error)?;
        transaction.commit().map_err(super::rows::database_error)?;
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

use nessa_local_database::rusqlite::OptionalExtension;
