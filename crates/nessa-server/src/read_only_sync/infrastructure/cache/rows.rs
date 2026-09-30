use crate::read_only_sync::application::{CacheError, CachePolicy, CachedProgress};
use nessa_local_database::rusqlite::{
    params, Connection, Error, ErrorCode, OptionalExtension, Row,
};
use nessa_sdk::infrastructure::session_storage::{
    TranscriptCheckpoint, MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES,
};
use nessa_sync::replication::domain::{Id, Scope, MAX_ID_BYTES};

// Storage conversion envelope; semantic acceptance remains with Id::new.
pub(super) const MAX_STORED_ID_BYTES: usize = 2 * MAX_ID_BYTES;

#[cfg(test)]
std::thread_local! {
    pub(super) static IDENTIFIER_TEXT_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn database_error(error: Error) -> CacheError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DiskFull) => CacheError::Quota,
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => CacheError::Corrupt,
        _ => CacheError::Unavailable,
    }
}

pub(super) fn number(row: &Row<'_>, index: usize) -> Result<u64, CacheError> {
    let bytes = row
        .get_ref(index)
        .map_err(database_error)?
        .as_blob()
        .map_err(|_| CacheError::Corrupt)?;
    let bytes = bytes.try_into().map_err(|_| CacheError::Corrupt)?;
    Ok(u64::from_be_bytes(bytes))
}

pub(super) fn identifier(row: &Row<'_>, index: usize) -> Result<Id, CacheError> {
    let text = row
        .get_ref(index)
        .map_err(database_error)?
        .as_str()
        .map_err(|_| CacheError::Corrupt)?;
    #[cfg(test)]
    IDENTIFIER_TEXT_READS.with(|count| count.set(count.get() + 1));
    Id::new(text).map_err(|_| CacheError::Corrupt)
}

pub(super) fn fenced(connection: &Connection, scope: &Scope) -> Result<bool, CacheError> {
    fenced_target(connection, scope.receiver(), scope.origin(), scope.stream())
}

pub(super) fn fenced_target(
    connection: &Connection,
    receiver: &Id,
    origin: &Id,
    conversation: &Id,
) -> Result<bool, CacheError> {
    connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM catalogue_deletions
             WHERE receiver = ?1 AND origin = ?2 AND conversation = ?3)",
            params![receiver.as_str(), origin.as_str(), conversation.as_str()],
            |row| row.get(0),
        )
        .map_err(database_error)
}

pub(super) fn progress(
    connection: &Connection,
    scope: &Scope,
) -> Result<Option<CachedProgress>, CacheError> {
    progress_target(connection, scope.receiver(), scope.origin(), scope.stream())
}

pub(super) fn progress_target(
    connection: &Connection,
    receiver: &Id,
    origin: &Id,
    stream: &Id,
) -> Result<Option<CachedProgress>, CacheError> {
    connection
        .query_row(
            "SELECT CASE WHEN octet_length(incarnation) <= ?4 THEN incarnation END,
                    CASE WHEN octet_length(schema_id) <= ?4 THEN schema_id END,
                    CASE WHEN octet_length(access_epoch) <= ?4 THEN access_epoch END,
                    downloaded, applied, facts, generation
             FROM transcript_progress WHERE receiver = ?1 AND origin = ?2 AND stream = ?3",
            params![
                receiver.as_str(),
                origin.as_str(),
                stream.as_str(),
                i64::try_from(MAX_STORED_ID_BYTES).map_err(|_| CacheError::InvalidPolicy)?
            ],
            |row| {
                Ok((|| {
                    let saved = CachedProgress {
                        scope: Scope::new(
                            receiver.clone(),
                            origin.clone(),
                            stream.clone(),
                            identifier(row, 0)?,
                            identifier(row, 1)?,
                            identifier(row, 2)?,
                        ),
                        downloaded: number(row, 3)?,
                        applied: number(row, 4)?,
                        facts: number(row, 5)?,
                        generation: number(row, 6)?,
                    };
                    if saved.generation == 0 || saved.applied > saved.downloaded {
                        return Err(CacheError::Corrupt);
                    }
                    Ok(saved)
                })())
            },
        )
        .optional()
        .map_err(database_error)?
        .transpose()
}

pub(super) fn checkpoint(
    connection: &Connection,
    scope: &Scope,
    policy: CachePolicy,
) -> Result<TranscriptCheckpoint, CacheError> {
    let maximum_chunks = policy
        .checkpoint_bytes()
        .div_ceil(MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES);
    let read_limit =
        i64::try_from(maximum_chunks.saturating_add(1)).map_err(|_| CacheError::InvalidPolicy)?;
    let mut statement = connection
        .prepare(
            "SELECT ordinal, length(payload) FROM transcript_checkpoints
             WHERE receiver = ?1 AND origin = ?2 AND stream = ?3
             ORDER BY ordinal LIMIT ?4",
        )
        .map_err(database_error)?;
    let metadata = statement
        .query_map(
            params![
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str(),
                read_limit
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    if metadata.is_empty() {
        return Err(CacheError::Corrupt);
    }
    if metadata.len() > maximum_chunks {
        return Err(CacheError::Quota);
    }
    let mut total = 0usize;
    for (index, &(ordinal, length)) in metadata.iter().enumerate() {
        let length = usize::try_from(length).map_err(|_| CacheError::Corrupt)?;
        if usize::try_from(ordinal).ok() != Some(index)
            || length == 0
            || length > MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES
            || (index + 1 != metadata.len() && length != MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES)
        {
            return Err(CacheError::Corrupt);
        }
        total = total.checked_add(length).ok_or(CacheError::Quota)?;
        if total > policy.checkpoint_bytes() {
            return Err(CacheError::Quota);
        }
    }
    let mut chunks = Vec::with_capacity(metadata.len());
    for (ordinal, expected_length) in metadata {
        let bytes: Vec<u8> = connection
            .query_row(
                "SELECT payload FROM transcript_checkpoints
                 WHERE receiver = ?1 AND origin = ?2 AND stream = ?3 AND ordinal = ?4
                   AND length(payload) = ?5",
                params![
                    scope.receiver().as_str(),
                    scope.origin().as_str(),
                    scope.stream().as_str(),
                    ordinal,
                    expected_length
                ],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        chunks.push(bytes);
    }
    TranscriptCheckpoint::from_chunks(chunks).map_err(super::records::transcript_error)
}

pub(super) fn save_progress(
    connection: &Connection,
    progress: &CachedProgress,
) -> Result<(), CacheError> {
    let scope = &progress.scope;
    connection
        .execute(
            "INSERT INTO transcript_progress
             (receiver, origin, stream, incarnation, schema_id, access_epoch,
              downloaded, applied, facts, generation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(receiver, origin, stream) DO UPDATE SET
               downloaded = excluded.downloaded, applied = excluded.applied,
               facts = excluded.facts, generation = excluded.generation",
            params![
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str(),
                scope.incarnation().as_str(),
                scope.schema().as_str(),
                scope.access_epoch().as_str(),
                progress.downloaded.to_be_bytes().as_slice(),
                progress.applied.to_be_bytes().as_slice(),
                progress.facts.to_be_bytes().as_slice(),
                progress.generation.to_be_bytes().as_slice()
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn save_checkpoint(
    connection: &Connection,
    scope: &Scope,
    checkpoint: &TranscriptCheckpoint,
    policy: CachePolicy,
) -> Result<(), CacheError> {
    let total = checkpoint.chunks().try_fold(0usize, |total, bytes| {
        total.checked_add(bytes.len()).ok_or(CacheError::Quota)
    })?;
    if total > policy.checkpoint_bytes() {
        return Err(CacheError::Quota);
    }
    connection
        .execute(
            "DELETE FROM transcript_checkpoints
             WHERE receiver = ?1 AND origin = ?2 AND stream = ?3",
            params![
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str()
            ],
        )
        .map_err(database_error)?;
    for (index, bytes) in checkpoint.chunks().enumerate() {
        connection
            .execute(
                "INSERT INTO transcript_checkpoints (receiver, origin, stream, ordinal, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    scope.receiver().as_str(),
                    scope.origin().as_str(),
                    scope.stream().as_str(),
                    i64::try_from(index).map_err(|_| CacheError::Quota)?,
                    bytes
                ],
            )
            .map_err(database_error)?;
    }
    Ok(())
}
