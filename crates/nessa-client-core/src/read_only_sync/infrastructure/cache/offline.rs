//! Stable target selection reads actual retained scopes in one SQLite snapshot.
use super::{catalogue_rows, rows, ReadOnlyCache};
use crate::read_only_sync::application::{
    offline::{SavedReads, SavedTranscript},
    CacheError, CachedCatalogueEntry, CachedCataloguePage, CachedProgress,
};
use nessa_local_database::rusqlite::{params, TransactionBehavior};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::{
    catalogue::{CatalogueProgress, EntryKey, MAX_CATALOGUE_ENTRIES, MAX_CATALOGUE_PAYLOAD_BYTES},
    domain::Id,
};
use uuid::Uuid;

impl SavedReads for ReadOnlyCache {
    fn catalogue_page(
        &mut self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
        after: Option<&EntryKey>,
        max_entries: usize,
    ) -> Result<Option<CachedCataloguePage>, CacheError> {
        self.retained_catalogue_page(receiver, origin, stream, after, max_entries)
    }

    fn transcript_view(
        &mut self,
        receiver: &Id,
        origin: &Id,
        conversation: &ConversationId,
        revision: Uuid,
    ) -> Result<SavedTranscript, CacheError> {
        self.retained_transcript_view(receiver, origin, conversation, revision)
    }
}

impl ReadOnlyCache {
    /// The one catalogue scope saved for `receiver`. A receiver reads one
    /// gateway's catalogue, so a second is damage.
    pub(crate) fn catalogue_scope_of(
        &mut self,
        receiver: &Id,
    ) -> Result<Option<nessa_sync::replication::domain::Scope>, CacheError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(rows::database_error)?;
        let targets = {
            let mut statement = tx
                .prepare(
                    "SELECT CASE WHEN octet_length(origin)<=?2 THEN origin END,
                            CASE WHEN octet_length(stream)<=?2 THEN stream END
                     FROM catalogue_progress WHERE receiver=?1 LIMIT 2",
                )
                .map_err(rows::database_error)?;
            let selected = statement
                .query_map(
                    params![receiver.as_str(), rows::MAX_STORED_ID_BYTES as i64],
                    |row| {
                        Ok((|| {
                            Ok((rows::identifier(row, 0)?, rows::identifier(row, 1)?))
                        })())
                    },
                )
                .map_err(rows::database_error)?;
            selected
                .map(|row| row.map_err(rows::database_error)?)
                .collect::<Result<Vec<(Id, Id)>, CacheError>>()?
        };
        let scope = match targets.as_slice() {
            [] => None,
            [(origin, stream)] => catalogue_rows::progress_target(&tx, receiver, origin, stream)?
                .map(|progress| progress.scope),
            _ => return Err(CacheError::Corrupt),
        };
        tx.commit().map_err(rows::database_error)?;
        Ok(scope)
    }

    pub(crate) fn retained_catalogue_progress(
        &self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
    ) -> Result<Option<CatalogueProgress>, CacheError> {
        catalogue_rows::progress_target(&self.connection, receiver, origin, stream)
    }

    pub(crate) fn retained_transcript_progress(
        &mut self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
    ) -> Result<Option<CachedProgress>, CacheError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(rows::database_error)?;
        if rows::fenced_target(&tx, receiver, origin, stream)? {
            return Err(CacheError::Fenced);
        }
        let progress = rows::progress_target(&tx, receiver, origin, stream)?;
        tx.commit().map_err(rows::database_error)?;
        Ok(progress)
    }

    pub(crate) fn retained_catalogue_page(
        &mut self,
        receiver: &Id,
        origin: &Id,
        stream: &Id,
        after: Option<&EntryKey>,
        max_entries: usize,
    ) -> Result<Option<CachedCataloguePage>, CacheError> {
        if max_entries == 0 || max_entries > MAX_CATALOGUE_ENTRIES {
            return Err(CacheError::InvalidPolicy);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(rows::database_error)?;
        let Some(progress) = catalogue_rows::progress_target(&tx, receiver, origin, stream)? else {
            return Ok(None);
        };
        let after_creation = after.map(|key| key.creation.to_be_bytes());
        let selected = {
            let mut statement = tx
                .prepare(
                    "SELECT CASE WHEN octet_length(entry_id)<=?4 THEN entry_id END,length(payload)
                FROM catalogue_entries WHERE receiver=?1 AND origin=?2 AND stream=?3
                AND (?5 IS NULL OR creation>?5 OR (creation=?5 AND entry_id>?6))
                ORDER BY creation,entry_id LIMIT ?7",
                )
                .map_err(rows::database_error)?;
            let selected = statement
                .query_map(
                    params![
                        receiver.as_str(),
                        origin.as_str(),
                        stream.as_str(),
                        rows::MAX_STORED_ID_BYTES as i64,
                        after_creation.as_ref().map(|value| value.as_slice()),
                        after.map(|key| key.id.as_str()),
                        (max_entries + 1) as i64
                    ],
                    |row| {
                        Ok((|| {
                            Ok((
                                rows::identifier(row, 0)?,
                                row.get::<_, i64>(1).map_err(rows::database_error)?,
                            ))
                        })())
                    },
                )
                .map_err(rows::database_error)?;
            selected
                .map(|row| row.map_err(rows::database_error)?)
                .collect::<Result<Vec<_>, CacheError>>()?
        };
        let has_more = selected.len() > max_entries;
        let mut total = 0usize;
        for (_, length) in selected.iter().take(max_entries) {
            let length = usize::try_from(*length).map_err(|_| CacheError::Corrupt)?;
            total = total.checked_add(length).ok_or(CacheError::Quota)?;
            if total > MAX_CATALOGUE_PAYLOAD_BYTES {
                return Err(CacheError::Quota);
            }
        }
        let mut entries = Vec::with_capacity(max_entries.min(selected.len()));
        for (id, _) in selected.into_iter().take(max_entries) {
            let (value, metadata) = catalogue_rows::entry_with_metadata(&tx, &progress.scope, &id)?
                .ok_or(CacheError::Corrupt)?;
            entries.push(CachedCatalogueEntry::new(value.manifest, metadata));
        }
        let next = if has_more {
            entries.last().map(|entry| entry.manifest().key.clone())
        } else {
            None
        };
        tx.commit().map_err(rows::database_error)?;
        Ok(Some(CachedCataloguePage::new(
            progress,
            entries.into_boxed_slice(),
            next,
        )))
    }
}
