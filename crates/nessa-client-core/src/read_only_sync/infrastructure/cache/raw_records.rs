use super::rows::{database_error, identifier, number, MAX_STORED_ID_BYTES};
use crate::read_only_sync::application::{CacheError, CachePolicy, CachedProgress};
use nessa_local_database::rusqlite::{ffi, params, Connection, Error, OptionalExtension};
use nessa_sdk::infrastructure::session_storage::{
    TranscriptFold, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
use nessa_sync::replication::domain::{CommitPlan, Id, Record, Scope};

pub(super) fn restore_suffix(
    connection: &Connection,
    progress: &CachedProgress,
    fold: &mut TranscriptFold,
    policy: CachePolicy,
) -> Result<(), CacheError> {
    let scope = &progress.scope;
    let limit = policy.suffix_page();
    if let Some(through) = fold.gap_through() {
        if through > progress.downloaded {
            return Err(CacheError::Corrupt);
        }
        fold.resume_downloaded(through)
            .map_err(super::records::transcript_error)?;
    }
    while fold.downloaded() < progress.downloaded {
        let mut statement = connection
            .prepare(
                "SELECT position, octet_length(record_id), length(payload) FROM transcript_records
                 WHERE receiver = ?1 AND origin = ?2 AND stream = ?3
                   AND position > ?4 AND position <= ?5 ORDER BY position LIMIT ?6",
            )
            .map_err(database_error)?;
        let metadata = statement
            .query_map(
                params![
                    scope.receiver().as_str(),
                    scope.origin().as_str(),
                    scope.stream().as_str(),
                    fold.downloaded().to_be_bytes().as_slice(),
                    progress.downloaded.to_be_bytes().as_slice(),
                    i64::try_from(limit.max_records()).map_err(|_| CacheError::InvalidPolicy)?
                ],
                |row| {
                    Ok((|| {
                        let position = number(row, 0)?;
                        let id_bytes: i64 = row.get(1).map_err(database_error)?;
                        if usize::try_from(id_bytes)
                            .map_or(true, |bytes| bytes > MAX_STORED_ID_BYTES)
                        {
                            return Err(CacheError::Corrupt);
                        }
                        let length: i64 = row.get(2).map_err(database_error)?;
                        let length = usize::try_from(length).map_err(|_| CacheError::Corrupt)?;
                        if length == 0 || length > MAX_PHYSICAL_RECORD_PAYLOAD_BYTES {
                            return Err(CacheError::Corrupt);
                        }
                        Ok((position, id_bytes, length))
                    })())
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if metadata.is_empty() {
            return Err(CacheError::Corrupt);
        }
        let mut bytes = 0usize;
        let mut batch = Vec::with_capacity(metadata.len());
        for (position, id_bytes, length) in metadata {
            if length > limit.max_record_bytes() {
                return Err(CacheError::Quota);
            }
            let next = bytes.checked_add(length).ok_or(CacheError::Quota)?;
            if next > limit.max_payload_bytes() {
                if batch.is_empty() {
                    return Err(CacheError::Quota);
                }
                break;
            }
            let (id, payload) = connection
                .query_row(
                    "SELECT record_id, payload FROM transcript_records
                     WHERE receiver = ?1 AND origin = ?2 AND stream = ?3 AND position = ?4
                       AND octet_length(record_id) = ?5 AND length(payload) = ?6",
                    params![
                        scope.receiver().as_str(),
                        scope.origin().as_str(),
                        scope.stream().as_str(),
                        position.to_be_bytes().as_slice(),
                        id_bytes,
                        i64::try_from(length).map_err(|_| CacheError::Corrupt)?
                    ],
                    |row| {
                        Ok((|| {
                            let id = identifier(row, 0)?;
                            let payload: Vec<u8> = row.get(1).map_err(database_error)?;
                            Ok((id, payload))
                        })())
                    },
                )
                .map_err(database_error)??;
            batch.push(Record {
                position,
                id,
                scope: scope.clone(),
                payload,
            });
            bytes = next;
        }
        fold.apply(&batch)
            .map_err(super::records::transcript_error)?;
    }
    if fold.downloaded() != progress.downloaded
        || fold.applied() != progress.applied
        || fold.fact_count() != progress.facts
    {
        return Err(CacheError::Corrupt);
    }
    fold.mark_stale();
    Ok(())
}

pub(super) fn exact_saved_plan(
    connection: &Connection,
    plan: &CommitPlan,
    retains: impl Fn(u64) -> bool,
) -> Result<bool, CacheError> {
    let scope = plan.expected().scope();
    for record in plan.records() {
        let exact = connection
            .query_row(
                "SELECT record_id = ?5, length(payload),
                        CASE WHEN length(payload) = ?6 THEN payload END
                 FROM transcript_records
                 WHERE receiver = ?1 AND origin = ?2 AND stream = ?3 AND position = ?4",
                params![
                    scope.receiver().as_str(),
                    scope.origin().as_str(),
                    scope.stream().as_str(),
                    record.position.to_be_bytes().as_slice(),
                    record.id.as_str(),
                    i64::try_from(record.payload.len()).map_err(|_| CacheError::Quota)?
                ],
                |row| {
                    Ok((|| {
                        let same_id: bool = row.get(0).map_err(database_error)?;
                        let length: i64 = row.get(1).map_err(database_error)?;
                        if !same_id || usize::try_from(length).ok() != Some(record.payload.len()) {
                            return Ok(false);
                        }
                        let payload = row
                            .get_ref(2)
                            .map_err(database_error)?
                            .as_blob()
                            .map_err(|_| CacheError::Corrupt)?;
                        Ok(payload == record.payload)
                    })())
                },
            )
            .optional()
            .map_err(database_error)?
            .transpose()?;
        // An unread span is not stored. Absence there is the saved plan.
        // A row that is present still has to match, and every retained
        // record has to be present and exact.
        if !retains(record.position) && exact.is_none() {
            continue;
        }
        if exact != Some(true) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn check_ids(connection: &Connection, plan: &CommitPlan) -> Result<(), CacheError> {
    let scope = plan.expected().scope();
    for record in plan.records() {
        let saved = connection
            .query_row(
                "SELECT position FROM transcript_records
                 WHERE receiver = ?1 AND origin = ?2 AND stream = ?3 AND record_id = ?4",
                params![
                    scope.receiver().as_str(),
                    scope.origin().as_str(),
                    scope.stream().as_str(),
                    record.id.as_str()
                ],
                |row| Ok(number(row, 0)),
            )
            .optional()
            .map_err(database_error)?
            .transpose()?;
        if saved.is_some_and(|position| position != record.position) {
            return Err(CacheError::ConflictingRecord);
        }
    }
    Ok(())
}

pub(super) fn insert_records(
    connection: &Connection,
    plan: &CommitPlan,
    retains: impl Fn(u64) -> bool,
) -> Result<(), CacheError> {
    let scope = plan.expected().scope();
    for record in plan.records() {
        if !retains(record.position) {
            continue;
        }
        connection
            .execute(
                "INSERT INTO transcript_records
                 (receiver, origin, stream, position, record_id, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    scope.receiver().as_str(),
                    scope.origin().as_str(),
                    scope.stream().as_str(),
                    record.position.to_be_bytes().as_slice(),
                    record.id.as_str(),
                    &record.payload
                ],
            )
            .map_err(|error| match &error {
                Error::SqliteFailure(code, _)
                    if matches!(
                        code.extended_code,
                        ffi::SQLITE_CONSTRAINT_PRIMARYKEY | ffi::SQLITE_CONSTRAINT_UNIQUE
                    ) =>
                {
                    CacheError::ConflictingRecord
                }
                _ => database_error(error),
            })?;
    }
    Ok(())
}

pub(super) fn clear_transcript(connection: &Connection, scope: &Scope) -> Result<(), CacheError> {
    clear_transcript_target(connection, scope.receiver(), scope.origin(), scope.stream())
}

pub(super) fn clear_transcript_target(
    connection: &Connection,
    receiver: &Id,
    origin: &Id,
    stream: &Id,
) -> Result<(), CacheError> {
    for table in ["transcript_records", "transcript_checkpoints"] {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE receiver = ?1 AND origin = ?2 AND stream = ?3"),
                params![receiver.as_str(), origin.as_str(), stream.as_str()],
            )
            .map_err(database_error)?;
    }
    connection
        .execute(
            "DELETE FROM transcript_progress
             WHERE receiver = ?1 AND origin = ?2 AND stream = ?3",
            params![receiver.as_str(), origin.as_str(), stream.as_str()],
        )
        .map_err(database_error)?;
    Ok(())
}
