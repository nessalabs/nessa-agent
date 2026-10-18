//! Rows the export cut keeps from this database. The journal is read here
//! because this store owns it.

use super::cached::CachedSql;
use std::path::Path;

use super::failed;
use crate::conversation::application::{ConversationError, ResourceKind};
use nessa_local_database::rusqlite::{Connection, Error, OpenFlags, Row};
use serde_json::{json, Value};

/// One party that no longer holds its grant: a revoke, or a receiver the
/// grant moved off. `capabilities` is what that party held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationFact {
    pub conversation_id: String,
    pub receiver_id: String,
    pub credential_id: String,
    /// The capabilities the revoke took away, as [`Capabilities::as_str`].
    pub capabilities: String,
    pub sequence: u64,
}

/// A row the cut can name but not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportSkip {
    Metadata { identity: String },
    Grant { identity: String },
}

/// One row the cut can name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedRecord {
    pub identity: String,
    pub bytes: Vec<u8>,
}

/// Tombstones and revocations from one metadata snapshot, the fences an
/// export appends while admission is still held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFences {
    pub tombstones: Vec<NamedRecord>,
    pub revocations: Vec<RevocationFact>,
}

/// A read transaction on the metadata database.
///
/// [`Self::open`] records `head:journal` and keeps that snapshot. [`Self::facts`]
/// reads rows from it after admission has been released.
pub struct MetadataSnapshot {
    connection: Connection,
    watermark: String,
}

impl MetadataSnapshot {
    /// Begin a read transaction. A missing or unreadable file is
    /// [`ConversationError::Metadata`]. The file is not created.
    pub fn open(path: &Path) -> Result<Self, ConversationError> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(failed)?;
        connection.execute_batch("BEGIN").map_err(failed)?;
        let watermark = watermark(&connection)?;
        Ok(Self {
            connection,
            watermark,
        })
    }

    /// Tombstones and revocations from this snapshot.
    ///
    /// A row that cannot be decoded or named fails the source. Leaving it
    /// out would look like a re-grant, and reconciliation would delete a
    /// fence already stored. [`Self::facts`] still names that row.
    pub fn fences(&self) -> Result<ExportFences, ConversationError> {
        let mut skipped = Vec::new();
        let tombstones = tombstones(&self.connection, &mut skipped)?;
        let revocations = revocation_rows(&self.connection, &mut skipped)?;
        if !skipped.is_empty() {
            return Err(ConversationError::Metadata);
        }
        Ok(ExportFences {
            tombstones,
            revocations,
        })
    }

    pub fn facts(&self) -> Result<ExportFacts, ConversationError> {
        let mut facts = read(&self.connection)?;
        facts.watermark = self.watermark.clone();
        Ok(facts)
    }
}

/// The metadata database at one read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFacts {
    /// Catalogue head and the latest journal sequence, `head:journal`.
    pub watermark: String,
    pub conversations: Vec<NamedRecord>,
    pub summaries: Vec<NamedRecord>,
    pub tombstones: Vec<NamedRecord>,
    pub grants: Vec<NamedRecord>,
    pub revocations: Vec<RevocationFact>,
    pub skipped: Vec<ExportSkip>,
}

fn watermark(connection: &Connection) -> Result<String, ConversationError> {
    let head: i64 = connection
        .cached_row(
            "SELECT COALESCE(MAX(head), 0) FROM catalogue_owners",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    let journal: i64 = connection
        .cached_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM share_grant_changes",
            [],
            |row| row.get(0),
        )
        .map_err(failed)?;
    Ok(format!("{head}:{journal}"))
}

pub(super) fn read(connection: &Connection) -> Result<ExportFacts, ConversationError> {
    let watermark = watermark(connection)?;
    let mut skipped = Vec::new();
    let conversations = json_rows(
        connection,
        "SELECT id, organization, owner, agent, model, approval_mode,
                creation_revision, change_revision
         FROM conversations ORDER BY id",
        &mut skipped,
        metadata_skip,
        conversation_row,
        "conversation",
    )?;
    let summaries = json_rows(
        connection,
        "SELECT conversation_id, title, preview, updated_at_ms, archived
         FROM summaries ORDER BY conversation_id",
        &mut skipped,
        metadata_skip,
        summary_row,
        "summary",
    )?;
    let tombstones = tombstones(connection, &mut skipped)?;
    let grants = json_rows(
        connection,
        &format!(
            "SELECT resource_id, principal_id, receiver_id, credential_id, capabilities, policy,
                    policy_revision
             FROM share_grants WHERE resource_kind = '{kind}'
             ORDER BY resource_id, principal_id",
            kind = ResourceKind::Conversation.as_str(),
        ),
        &mut skipped,
        grant_skip,
        grant_row,
        "grant",
    )?;
    let revocations = revocation_rows(connection, &mut skipped)?;
    Ok(ExportFacts {
        watermark,
        conversations,
        summaries,
        tombstones,
        grants,
        revocations,
        skipped,
    })
}

fn tombstones(
    connection: &Connection,
    skipped: &mut Vec<ExportSkip>,
) -> Result<Vec<NamedRecord>, ConversationError> {
    json_rows(
        connection,
        "SELECT conversation_id, erased FROM deletions ORDER BY conversation_id",
        skipped,
        metadata_skip,
        tombstone_row,
        "tombstone",
    )
}

fn conversation_row(row: &Row<'_>) -> Result<Value, Error> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization": row.get::<_, String>(1)?,
        "owner": row.get::<_, String>(2)?,
        "agent": row.get::<_, String>(3)?,
        "model": row.get::<_, String>(4)?,
        "approvalMode": row.get::<_, String>(5)?,
        "creationRevision": row.get::<_, i64>(6)?,
        "changeRevision": row.get::<_, i64>(7)?,
    }))
}

fn summary_row(row: &Row<'_>) -> Result<Value, Error> {
    Ok(json!({
        "conversationId": row.get::<_, String>(0)?,
        "title": row.get::<_, Option<String>>(1)?,
        "preview": row.get::<_, Option<String>>(2)?,
        "updatedAtMs": row.get::<_, i64>(3)?,
        "archived": row.get::<_, i64>(4)? == 1,
    }))
}

fn tombstone_row(row: &Row<'_>) -> Result<Value, Error> {
    Ok(json!({
        "conversationId": row.get::<_, String>(0)?,
        "erased": row.get::<_, i64>(1)? == 1,
    }))
}

fn grant_row(row: &Row<'_>) -> Result<Value, Error> {
    let capabilities: String = row.get(4)?;
    if !known_capabilities(&capabilities) {
        return Ok(Value::Null);
    }
    let mut value = json!({
        "conversationId": row.get::<_, String>(0)?,
        "resourceKind": ResourceKind::Conversation.as_str(),
        "principalId": row.get::<_, String>(1)?,
        "credentialId": row.get::<_, String>(3)?,
        "capabilities": capabilities,
        "policy": row.get::<_, Option<String>>(5)?,
        "policyRevision": row.get::<_, i64>(6)?,
    });
    if let Some(receiver) = row.get::<_, Option<String>>(2)? {
        value["receiverId"] = Value::String(receiver);
    }
    Ok(value)
}

fn json_rows(
    connection: &Connection,
    sql: &str,
    skipped: &mut Vec<ExportSkip>,
    skip: fn(String) -> ExportSkip,
    decode: fn(&Row<'_>) -> Result<Value, Error>,
    prefix: &str,
) -> Result<Vec<NamedRecord>, ConversationError> {
    let mut statement = connection.prepare(sql).map_err(failed)?;
    let mut query = statement.query([]).map_err(failed)?;
    let mut records = Vec::new();
    let mut index = 0u64;
    while let Some(row) = query.next().map_err(failed)? {
        index += 1;
        let value = match decode(row) {
            Ok(value) => value,
            Err(error) if super::damaged(&error) => {
                skipped.push(skip(format!("row-{index}")));
                continue;
            }
            Err(error) => return Err(failed(error)),
        };
        let Some(identity) = identity_of(prefix, &value) else {
            skipped.push(skip(format!("row-{index}")));
            continue;
        };
        records.push(NamedRecord {
            identity,
            bytes: serde_json::to_vec(&value).map_err(|_| ConversationError::Metadata)?,
        });
    }
    Ok(records)
}

fn revocation_rows(
    connection: &Connection,
    skipped: &mut Vec<ExportSkip>,
) -> Result<Vec<RevocationFact>, ConversationError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT resource_id, principal_id, receiver_id, credential_id, capabilities, sequence
                 FROM (
                     SELECT j.resource_id, j.principal_id, j.receiver_id, j.credential_id,
                            j.before AS capabilities, j.sequence
                     FROM share_grant_changes AS j
                     WHERE j.resource_kind = '{kind}'
                       AND j.after = 'none'
                       AND j.sequence = (
                           SELECT MAX(j2.sequence) FROM share_grant_changes AS j2
                           WHERE j2.resource_kind = j.resource_kind
                             AND j2.resource_id = j.resource_id
                             AND j2.principal_id = j.principal_id
                       )
                     UNION ALL
                     SELECT j.resource_id, j.principal_id, j.receiver_id, j.credential_id,
                            j.after AS capabilities, j.sequence
                     FROM share_grant_changes AS j
                     WHERE j.resource_kind = '{kind}'
                       AND j.receiver_id IS NOT NULL
                       AND j.after <> 'none'
                       AND j.sequence = (
                           SELECT MAX(j2.sequence) FROM share_grant_changes AS j2
                           WHERE j2.resource_kind = j.resource_kind
                             AND j2.resource_id = j.resource_id
                             AND j2.receiver_id = j.receiver_id
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM share_grants AS g
                           WHERE g.resource_kind = j.resource_kind
                             AND g.resource_id = j.resource_id
                             AND g.receiver_id = j.receiver_id
                       )
                 )
                 ORDER BY sequence",
            kind = ResourceKind::Conversation.as_str(),
        ))
        .map_err(failed)?;
    let mut query = statement.query([]).map_err(failed)?;
    let mut found = Vec::new();
    let mut index = 0u64;
    while let Some(row) = query.next().map_err(failed)? {
        index += 1;
        let parsed = (|| {
            let conversation_id: String = row.get(0)?;
            let principal_id: String = row.get(1)?;
            let receiver_id: Option<String> = row.get(2)?;
            let credential_id: String = row.get(3)?;
            let capabilities: String = row.get(4)?;
            let sequence: i64 = row.get(5)?;
            // A paired grant is fenced by its receiver. A grant that stores
            // none is fenced by its principal, in the same field: the
            // deletion line still names one party.
            let receiver_id = receiver_id.unwrap_or(principal_id);
            Ok::<_, Error>((
                conversation_id,
                receiver_id,
                credential_id,
                capabilities,
                sequence,
            ))
        })();
        let Ok((conversation_id, receiver_id, credential_id, capabilities, sequence)) = parsed
        else {
            skipped.push(ExportSkip::Grant {
                identity: format!("revoke-{index}"),
            });
            continue;
        };
        let Some(sequence) = u64::try_from(sequence)
            .ok()
            .filter(|sequence| *sequence > 0)
        else {
            skipped.push(ExportSkip::Grant {
                identity: format!("revoke-{index}"),
            });
            continue;
        };
        if !known_capabilities(&capabilities)
            || !nameable(&conversation_id)
            || !nameable(&receiver_id)
            || !nameable(&credential_id)
        {
            skipped.push(ExportSkip::Grant {
                identity: format!("revoke-{index}"),
            });
            continue;
        }
        found.push(RevocationFact {
            conversation_id,
            receiver_id,
            credential_id,
            capabilities,
            sequence,
        });
    }
    Ok(found)
}

fn identity_of(prefix: &str, value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let id = value
        .get("id")
        .or_else(|| value.get("conversationId"))
        .and_then(Value::as_str)?;
    let extra = value
        .get("receiverId")
        .and_then(Value::as_str)
        .or_else(|| value.get("principalId").and_then(Value::as_str))
        .map(|party| format!(":{party}"))
        .unwrap_or_default();
    let identity = format!("{prefix}-{id}{extra}");
    nameable(&identity).then_some(identity)
}

fn metadata_skip(identity: String) -> ExportSkip {
    ExportSkip::Metadata { identity }
}

fn grant_skip(identity: String) -> ExportSkip {
    ExportSkip::Grant { identity }
}

fn known_capabilities(capabilities: &str) -> bool {
    matches!(capabilities, "read" | "read+comment" | "read+comment+drive")
}

fn nameable(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|byte| byte >= 0x20)
}
