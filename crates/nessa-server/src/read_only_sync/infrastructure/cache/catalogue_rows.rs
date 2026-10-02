//! Bounded physical catalogue reads; core owns semantic progress and revision rules.
use super::rows;
use crate::conversation::application::CatalogueMetadata;
use crate::conversation::domain::ConversationId;
use crate::conversation::infrastructure::catalogue_payload;
use crate::read_only_sync::application::CacheError;
use nessa_local_database::rusqlite::types::ValueRef;
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension};
use nessa_sync::replication::catalogue::{
    validate_catalogue_progress, validate_manifest_entry, validate_resolved, CataloguePass,
    CatalogueProgress, EntryKey, ManifestEntry, ResolvedEntry, MAX_CATALOGUE_PAYLOAD_BYTES,
};
use nessa_sync::replication::domain::{Id, Scope};
#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
std::thread_local! {
    pub(super) static PAYLOAD_READS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn progress(
    connection: &Connection,
    scope: &Scope,
) -> Result<Option<CatalogueProgress>, CacheError> {
    progress_target(connection, scope.receiver(), scope.origin(), scope.stream())
}

pub(super) fn progress_target(
    connection: &Connection,
    receiver: &Id,
    origin: &Id,
    stream: &Id,
) -> Result<Option<CatalogueProgress>, CacheError> {
    connection
        .query_row(
            "SELECT CASE WHEN octet_length(incarnation)<=?4 THEN incarnation END,
                CASE WHEN octet_length(schema_id)<=?4 THEN schema_id END,
                CASE WHEN octet_length(access_epoch)<=?4 THEN access_epoch END,
                completed,generation,active_boundary,cursor_creation,
                CASE WHEN octet_length(cursor_id)<=?4 THEN cursor_id END,
                cursor_id IS NOT NULL
         FROM catalogue_progress WHERE receiver=?1 AND origin=?2 AND stream=?3",
            params![
                receiver.as_str(),
                origin.as_str(),
                stream.as_str(),
                rows::MAX_STORED_ID_BYTES as i64
            ],
            |row| {
                Ok((|| {
                    let scope = Scope::new(
                        receiver.clone(),
                        origin.clone(),
                        stream.clone(),
                        rows::identifier(row, 0)?,
                        rows::identifier(row, 1)?,
                        rows::identifier(row, 2)?,
                    );
                    let completed = rows::number(row, 3)?;
                    let generation = rows::number(row, 4)?;
                    let boundary = if matches!(
                        row.get_ref(5).map_err(rows::database_error)?,
                        ValueRef::Null
                    ) {
                        None
                    } else {
                        Some(rows::number(row, 5)?)
                    };
                    let cursor_creation = if matches!(
                        row.get_ref(6).map_err(rows::database_error)?,
                        ValueRef::Null
                    ) {
                        None
                    } else {
                        Some(rows::number(row, 6)?)
                    };
                    let cursor_present: bool = row.get(8).map_err(rows::database_error)?;
                    let cursor = match (cursor_creation, cursor_present) {
                        (None, false) => None,
                        (Some(creation), true) => Some(EntryKey {
                            creation,
                            id: rows::identifier(row, 7)?,
                        }),
                        _ => return Err(CacheError::Corrupt),
                    };
                    if boundary.is_none() && cursor.is_some() {
                        return Err(CacheError::Corrupt);
                    }
                    let active = boundary.map(|boundary| CataloguePass {
                        scope: scope.clone(),
                        completed,
                        generation,
                        boundary,
                        cursor,
                    });
                    let progress = CatalogueProgress {
                        scope,
                        completed,
                        generation,
                        active,
                    };
                    validate_catalogue_progress(&progress)
                        .map_err(CacheError::CatalogueProgress)?;
                    Ok(progress)
                })())
            },
        )
        .optional()
        .map_err(rows::database_error)?
        .transpose()
}

pub(super) fn exact_progress(
    connection: &Connection,
    scope: &Scope,
) -> Result<Option<CatalogueProgress>, CacheError> {
    let saved = progress(connection, scope)?;
    if let Some(saved) = &saved {
        if saved.scope != *scope {
            return Err(CacheError::Scope {
                saved: Box::new(saved.scope.clone()),
                requested: Box::new(scope.clone()),
            });
        }
    }
    Ok(saved)
}

pub(super) fn save_progress(
    connection: &Connection,
    progress: &CatalogueProgress,
) -> Result<(), CacheError> {
    let scope = &progress.scope;
    let boundary = progress
        .active
        .as_ref()
        .map(|pass| pass.boundary.to_be_bytes());
    let cursor = progress
        .active
        .as_ref()
        .and_then(|pass| pass.cursor.as_ref());
    let creation = cursor.map(|key| key.creation.to_be_bytes());
    connection.execute(
        "INSERT INTO catalogue_progress(receiver,origin,stream,incarnation,schema_id,access_epoch,completed,generation,active_boundary,cursor_creation,cursor_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(receiver,origin,stream) DO UPDATE SET incarnation=excluded.incarnation,schema_id=excluded.schema_id,access_epoch=excluded.access_epoch,completed=excluded.completed,generation=excluded.generation,active_boundary=excluded.active_boundary,cursor_creation=excluded.cursor_creation,cursor_id=excluded.cursor_id",
        params![scope.receiver().as_str(),scope.origin().as_str(),scope.stream().as_str(),scope.incarnation().as_str(),scope.schema().as_str(),scope.access_epoch().as_str(),progress.completed.to_be_bytes().as_slice(),progress.generation.to_be_bytes().as_slice(),boundary.as_ref().map(|v|v.as_slice()),creation.as_ref().map(|v|v.as_slice()),cursor.map(|key|key.id.as_str())],
    ).map_err(rows::database_error)?;
    Ok(())
}

pub(super) fn entry(
    connection: &Connection,
    scope: &Scope,
    id: &Id,
) -> Result<Option<ResolvedEntry>, CacheError> {
    entry_with_metadata(connection, scope, id).map(|entry| entry.map(|(value, _)| value))
}

pub(super) fn entry_with_metadata(
    connection: &Connection,
    scope: &Scope,
    id: &Id,
) -> Result<Option<(ResolvedEntry, Option<CatalogueMetadata>)>, CacheError> {
    let saved = connection
        .query_row(
            "SELECT creation,revision,deleted,length(payload)
         FROM catalogue_entries WHERE receiver=?1 AND origin=?2 AND stream=?3 AND entry_id=?4",
            params![
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str(),
                id.as_str()
            ],
            |row| {
                Ok((|| {
                    let length: i64 = row.get(3).map_err(rows::database_error)?;
                    if usize::try_from(length).map_or(true, |n| n > MAX_CATALOGUE_PAYLOAD_BYTES) {
                        return Err(CacheError::Quota);
                    }
                    let deleted: i64 = row.get(2).map_err(rows::database_error)?;
                    let deleted = match deleted {
                        0 => false,
                        1 => true,
                        _ => return Err(CacheError::Corrupt),
                    };
                    let manifest = ManifestEntry {
                        key: EntryKey {
                            creation: rows::number(row, 0)?,
                            id: id.clone(),
                        },
                        revision: rows::number(row, 1)?,
                        deleted,
                    };
                    validate_manifest_entry(&manifest).map_err(CacheError::CatalogueValidation)?;
                    Ok((manifest, length))
                })())
            },
        )
        .optional()
        .map_err(rows::database_error)?
        .transpose()?;
    let Some((manifest, length)) = saved else {
        return Ok(None);
    };
    let fenced = fence_state(connection, scope, &manifest)?;
    if manifest.deleted && !fenced {
        return Err(CacheError::Corrupt);
    }
    // The second query acquires only the descriptor and length admitted above.
    let payload = connection
        .query_row(
            "SELECT payload FROM catalogue_entries
         WHERE receiver=?1 AND origin=?2 AND stream=?3 AND entry_id=?4
           AND creation=?5 AND revision=?6 AND deleted=?7 AND length(payload)=?8",
            params![
                scope.receiver().as_str(),
                scope.origin().as_str(),
                scope.stream().as_str(),
                id.as_str(),
                manifest.key.creation.to_be_bytes().as_slice(),
                manifest.revision.to_be_bytes().as_slice(),
                i64::from(manifest.deleted),
                length
            ],
            |row| {
                #[cfg(test)]
                PAYLOAD_READS.with(|count| count.set(count.get() + 1));
                row.get(0)
            },
        )
        .optional()
        .map_err(rows::database_error)?
        .ok_or(CacheError::Corrupt)?;
    let value = ResolvedEntry { manifest, payload };
    validate_resolved(&value.manifest, &value, MAX_CATALOGUE_PAYLOAD_BYTES)
        .map_err(CacheError::CatalogueValidation)?;
    let metadata = metadata(&value)?;
    Ok(Some((value, metadata)))
}

/// A retained fence survives catalogue incarnation and progress replacement.
pub(super) fn fence_state(
    connection: &Connection,
    scope: &Scope,
    manifest: &ManifestEntry,
) -> Result<bool, CacheError> {
    let fenced = rows::fenced_target(
        connection,
        scope.receiver(),
        scope.origin(),
        &manifest.key.id,
    )?;
    if !manifest.deleted && fenced {
        return Err(CacheError::Fenced);
    }
    Ok(fenced)
}

pub(super) fn metadata(value: &ResolvedEntry) -> Result<Option<CatalogueMetadata>, CacheError> {
    if value.manifest.deleted {
        ConversationId::new(value.manifest.key.id.as_str())
            .map_err(|_| CacheError::CatalogueMetadata)?;
        Ok(None)
    } else {
        catalogue_payload::decode(value.manifest.key.id.as_str(), &value.payload)
            .map(Some)
            .map_err(|_| CacheError::CatalogueMetadata)
    }
}

pub(super) fn save_entry(
    connection: &Connection,
    scope: &Scope,
    value: &ResolvedEntry,
) -> Result<(), CacheError> {
    connection.execute(
        "INSERT INTO catalogue_entries(receiver,origin,stream,entry_id,creation,revision,deleted,payload) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(receiver,origin,stream,entry_id) DO UPDATE SET revision=excluded.revision,deleted=excluded.deleted,payload=excluded.payload",
        params![scope.receiver().as_str(),scope.origin().as_str(),scope.stream().as_str(),value.manifest.key.id.as_str(),value.manifest.key.creation.to_be_bytes().as_slice(),value.manifest.revision.to_be_bytes().as_slice(),i64::from(value.manifest.deleted),&value.payload],
    ).map_err(rows::database_error)?;
    Ok(())
}
