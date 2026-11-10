//! Put an accepted restore where the gateway's conversation service reads.
//!
//! The sealed tree stays the backup. Metadata rows and session streams are
//! copied into the namespace the service opens, which is not that tree.

use std::fs;
use std::path::Path;

use nessa_local_database::rusqlite::{params, Connection};
use nessa_local_storage::create_directory;

use crate::backup::domain::{decode_deferred, JOURNAL_HEADER};

const METADATA_SCHEMA: &str = include_str!("../conversation/infrastructure/schema.sql");

/// `namespace` is itself a staged or accepted restore tree.
pub(crate) fn namespace_is_restore_root(namespace: &Path) -> bool {
    let Ok(text) = fs::read_to_string(namespace.join("journal.txt")) else {
        return false;
    };
    text.starts_with(JOURNAL_HEADER)
}

/// Copy restored conversations into `namespace/conversations` before the
/// service opens that directory.
pub(crate) fn install_serving_stores(namespace: &Path, live: &Path) -> Result<(), String> {
    let conversations = namespace.join("conversations");
    create_directory(&conversations).map_err(|error| error.to_string())?;
    create_directory(&conversations.join("sessions")).map_err(|error| error.to_string())?;
    install_metadata(&conversations.join("metadata.sqlite3"), live)?;
    let serving = conversations.join("sessions").join("records.sqlite3");
    let canonical = live
        .join("records")
        .join("canonical-records")
        .join("records.sqlite3");
    if canonical.is_file() && !serving.exists() {
        fs::copy(&canonical, &serving).map_err(|error| error.to_string())?;
    }
    for id in deferred_ids(live)? {
        let published = live.join("sessions").join(format!("{id}.sqlite3"));
        if published.is_file() {
            merge_stream(&serving, &published, &id)?;
        }
    }
    Ok(())
}

/// Copy one fetched conversation into the session database the service reads.
pub(crate) fn merge_published_session(
    namespace: &Path,
    live: &Path,
    conversation_id: &str,
) -> Result<(), String> {
    let published = live
        .join("sessions")
        .join(format!("{conversation_id}.sqlite3"));
    if !published.is_file() {
        return Err("fetched session was not published".into());
    }
    let serving = namespace
        .join("conversations")
        .join("sessions")
        .join("records.sqlite3");
    if let Some(parent) = serving.parent() {
        create_directory(parent).map_err(|error| error.to_string())?;
    }
    merge_stream(&serving, &published, conversation_id)
}

fn deferred_ids(live: &Path) -> Result<Vec<String>, String> {
    let path = live.join("deferred.txt");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    decode_deferred(&text)
        .map(|lines| lines.into_iter().map(|line| line.conversation_id).collect())
        .map_err(|error| error.cause().to_owned())
}

fn install_metadata(path: &Path, live: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch(METADATA_SCHEMA)
        .map_err(|error| error.to_string())?;
    let mut rows = metadata_rows(live)?;
    for id in deferred_ids(live)? {
        if !rows.iter().any(|row| row.id == id) {
            rows.push(MetadataRow {
                id,
                organization: "restored".into(),
                owner: "restored".into(),
                agent: "restored".into(),
                model: "restored".into(),
                approval_mode: "ask".into(),
                title: None,
            });
        }
    }
    for row in rows {
        connection
            .execute(
                "INSERT OR IGNORE INTO catalogue_owners (organization, owner, head) VALUES (?1, ?2, 1)",
                params![row.organization, row.owner],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT OR IGNORE INTO conversations (
                    id, organization, owner, creator_surface, creation_action,
                    creation_requested_at_ms, agent, model, approval_mode,
                    creation_revision, change_revision
                 ) VALUES (?1, ?2, ?3, 'restore', 'backup.restore.accept', 0, ?4, ?5, ?6, 1, 1)",
                params![
                    row.id,
                    row.organization,
                    row.owner,
                    row.agent,
                    row.model,
                    row.approval_mode
                ],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT OR IGNORE INTO summaries (
                    conversation_id, title, preview, updated_at_ms, archived
                 ) VALUES (?1, ?2, NULL, 0, 0)",
                params![row.id, row.title],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

struct MetadataRow {
    id: String,
    organization: String,
    owner: String,
    agent: String,
    model: String,
    approval_mode: String,
    title: Option<String>,
}

fn metadata_rows(live: &Path) -> Result<Vec<MetadataRow>, String> {
    let directory = live.join("records").join("conversation-metadata");
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(id) = value
            .get("id")
            .or_else(|| value.get("conversationId"))
            .and_then(|id| id.as_str())
        else {
            continue;
        };
        if value.get("erased").is_some() && value.get("agent").is_none() {
            continue;
        }
        rows.push(MetadataRow {
            id: id.to_owned(),
            organization: text(&value, "organization").unwrap_or_else(|| "restored".into()),
            owner: text(&value, "owner").unwrap_or_else(|| "restored".into()),
            agent: text(&value, "agent").unwrap_or_else(|| "restored".into()),
            model: text(&value, "model").unwrap_or_else(|| "restored".into()),
            approval_mode: match text(&value, "approvalMode").as_deref() {
                Some(mode @ ("ask" | "auto" | "full")) => mode.to_owned(),
                _ => "ask".into(),
            },
            title: text(&value, "title"),
        });
    }
    Ok(rows)
}

fn text(value: &serde_json::Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(|item| item.as_str())
        .map(str::to_owned)
}

fn merge_stream(serving: &Path, published: &Path, conversation_id: &str) -> Result<(), String> {
    if !serving.exists() {
        fs::copy(published, serving).map_err(|error| error.to_string())?;
        return Ok(());
    }
    let published = published
        .to_str()
        .ok_or_else(|| "session path is not Unicode".to_owned())?;
    let connection = Connection::open(serving).map_err(|error| error.to_string())?;
    connection
        .execute("ATTACH DATABASE ?1 AS fetched", params![published])
        .map_err(|error| error.to_string())?;
    let present: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM event_streams WHERE public_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if present == 0 {
        connection
            .execute(
                "INSERT INTO event_streams (public_id, incarnation, floor, tail, retired)
                 SELECT public_id, incarnation, floor, tail, COALESCE(retired, 0)
                 FROM fetched.event_streams WHERE public_id = ?1",
                params![conversation_id],
            )
            .map_err(|error| error.to_string())?;
        let key: i64 = connection
            .query_row(
                "SELECT stream_key FROM event_streams WHERE public_id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT OR IGNORE INTO event_stream_names (
                    public_id, latest_incarnation, active_stream_key
                 )
                 SELECT public_id, latest_incarnation, ?2
                 FROM fetched.event_stream_names WHERE public_id = ?1",
                params![conversation_id, key],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT INTO event_records (
                    stream_key, offset, event_id, schema_id, schema_version, payload
                 )
                 SELECT ?2, offset, event_id, schema_id, schema_version, payload
                 FROM fetched.event_records
                 WHERE stream_key = (
                    SELECT stream_key FROM fetched.event_streams WHERE public_id = ?1
                 )",
                params![conversation_id, key],
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute("DETACH DATABASE fetched", [])
        .map_err(|error| error.to_string())?;
    Ok(())
}
