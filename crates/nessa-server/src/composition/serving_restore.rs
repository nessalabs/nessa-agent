//! Put an accepted restore where the gateway's conversation service reads.
//!
//! The sealed tree stays the backup. Metadata rows and session streams are
//! copied into the namespace the service opens, which is not that tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use nessa_local_database::rusqlite::{params, Connection, TransactionBehavior};
use nessa_local_storage::create_directory;

use crate::backup::domain::{decode_deferred, decode_identity, Fingerprint, JOURNAL_HEADER};

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
    if restore_install_matches(namespace, live) {
        return Ok(());
    }
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
        publish_copy(&canonical, &serving)?;
    }
    for id in deferred_ids(live)? {
        let published = live.join("sessions").join(format!("{id}.sqlite3"));
        if published.is_file() {
            merge_stream(&serving, &published, &id)?;
        }
    }
    write_install_marker(&conversations, live)
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

/// The local owner accepted this restore, so the catalogue is theirs.
///
/// Automatic provisioning mints a new organization and principal. Rows that
/// still name the backup's owner are invisible to that credential. One update
/// binds every restored conversation to the credential that will list them.
pub(crate) fn bind_restored_owner(
    path: &Path,
    organization: &str,
    owner: &str,
) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if count == 0 {
        transaction.commit().map_err(|error| error.to_string())?;
        return Ok(());
    }
    let head: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(change_revision), 1) FROM conversations",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO catalogue_owners (organization, owner, head) VALUES (?1, ?2, ?3)",
            params![organization, owner, head],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE conversations SET organization = ?1, owner = ?2",
            params![organization, owner],
        )
        .map_err(|error| error.to_string())?;
    // The tombstone names the same owner as the row it fences. Leaving the
    // backup's owner on the deletion would make the row unreadable.
    transaction
        .execute(
            "UPDATE deletions SET organization = ?1, initiator = ?2",
            params![organization, owner],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM catalogue_owners WHERE organization != ?1 OR owner != ?2",
            params![organization, owner],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE catalogue_owners SET head = ?3 WHERE organization = ?1 AND owner = ?2",
            params![organization, owner, head],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

const OWNER_HEADER: &str = "nessa-restore-owner/1";

/// Bind restored rows once. A later startup leaves conversations created
/// after that first bind with the owner who created them.
pub(crate) fn bind_installed_owner(
    namespace: &Path,
    organization: &str,
    owner: &str,
) -> Result<(), String> {
    let conversations = namespace.join("conversations");
    let marker = conversations.join("restore-owner.txt");
    if fs::read_to_string(&marker)
        .ok()
        .is_some_and(|text| text.lines().next() == Some(OWNER_HEADER))
    {
        return Ok(());
    }
    bind_restored_owner(&conversations.join("metadata.sqlite3"), organization, owner)?;
    fs::write(&marker, format!("{OWNER_HEADER}\n")).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Copy `source` to `destination` only when the copy finishes.
///
/// An interrupted copy leaves `destination.partial`. The next startup does not
/// treat that fragment as the serving database.
fn publish_copy(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Ok(());
    }
    let mut partial_name = destination.as_os_str().to_owned();
    partial_name.push(".partial");
    let partial = PathBuf::from(partial_name);
    let _ = fs::remove_file(&partial);
    if let Err(error) = fs::copy(source, &partial) {
        let _ = fs::remove_file(&partial);
        return Err(error.to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(&partial, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&partial);
            return Err(error.to_string());
        }
    }
    if let Err(error) = fs::rename(&partial, destination) {
        let _ = fs::remove_file(&partial);
        return Err(error.to_string());
    }
    Ok(())
}

const INSTALL_HEADER: &str = "nessa-restore-install/1";

/// Whether `namespace` already holds the serving stores installed from `live`.
pub(crate) fn restore_install_matches(namespace: &Path, live: &Path) -> bool {
    let Ok(digest) = journal_digest(live) else {
        return false;
    };
    marker_digest(&namespace.join("conversations")).as_deref() == Some(digest.as_str())
}

/// The serving database already holds this conversation's stream.
pub(crate) fn serving_has_stream(namespace: &Path, conversation_id: &str) -> bool {
    let path = namespace
        .join("conversations")
        .join("sessions")
        .join("records.sqlite3");
    if !path.is_file() {
        return false;
    }
    let Ok(connection) = Connection::open_with_flags(
        &path,
        nessa_local_database::rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return false;
    };
    connection
        .query_row(
            "SELECT COUNT(*) FROM event_streams WHERE public_id = ?1",
            [conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0
}

fn journal_digest(live: &Path) -> Result<String, String> {
    let bytes = fs::read(live.join("journal.txt")).map_err(|error| error.to_string())?;
    Ok(Fingerprint::of(&bytes).hex())
}

fn marker_digest(conversations: &Path) -> Option<String> {
    let text = fs::read_to_string(conversations.join("restore-install.txt")).ok()?;
    let mut lines = text.lines();
    if lines.next() != Some(INSTALL_HEADER) {
        return None;
    }
    lines.next().map(str::to_owned)
}

fn write_install_marker(conversations: &Path, live: &Path) -> Result<(), String> {
    let Ok(digest) = journal_digest(live) else {
        return Ok(());
    };
    let path = conversations.join("restore-install.txt");
    fs::write(&path, format!("{INSTALL_HEADER}\n{digest}\n")).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
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
    // The catalogue is published by rename. A stop while the rows are being
    // written leaves only the partial file, so the next startup installs again
    // instead of treating an empty database as finished.
    let mut partial_name = path.as_os_str().to_owned();
    partial_name.push(".partial");
    let partial = PathBuf::from(partial_name);
    let _ = fs::remove_file(&partial);
    let installed = write_metadata(&partial, live);
    if let Err(error) = installed {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    fs::rename(&partial, path).map_err(|error| error.to_string())
}

fn write_metadata(path: &Path, live: &Path) -> Result<(), String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .execute_batch(METADATA_SCHEMA)
        .map_err(|error| error.to_string())?;
    let restored = metadata_rows(live)?;
    let mut rows = restored.rows;
    for fence in &restored.fences {
        if nessa_protocol::conversation::domain::ConversationId::new(&fence.id).is_err() {
            continue;
        }
        if rows.iter().any(|row| row.id == fence.id) {
            continue;
        }
        // The staged tree keeps the tombstone and omits the conversation it
        // fences. The serving catalogue needs the row the deletion references.
        rows.push(MetadataRow {
            id: fence.id.clone(),
            organization: "restored".into(),
            owner: "restored".into(),
            agent: "restored".into(),
            model: "restored".into(),
            approval_mode: "ask".into(),
            creation_revision: 1,
            change_revision: 1,
            title: None,
            preview: None,
            updated_at_ms: 0,
            archived: 0,
            record_summary: false,
        });
    }
    for id in deferred_ids(live)? {
        // A deferred stream is not a conversation unless its id is one.
        // Command-control streams (`nessa:commands:…`) share an oversized
        // session file and must not become catalogue rows: pagination rejects
        // an id that is not a conversation UUID.
        if nessa_protocol::conversation::domain::ConversationId::new(&id).is_err() {
            continue;
        }
        if !rows.iter().any(|row| row.id == id) {
            rows.push(MetadataRow {
                id,
                organization: "restored".into(),
                owner: "restored".into(),
                agent: "restored".into(),
                model: "restored".into(),
                approval_mode: "ask".into(),
                creation_revision: 1,
                change_revision: 1,
                title: None,
                preview: None,
                updated_at_ms: 0,
                archived: 0,
                record_summary: true,
            });
        }
    }
    let transaction = connection
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let mut owner_heads = BTreeMap::<(String, String), i64>::new();
    for row in &rows {
        let head = owner_heads
            .entry((row.organization.clone(), row.owner.clone()))
            .or_insert(0);
        *head = (*head).max(row.change_revision);
    }
    for ((organization, owner), head) in &owner_heads {
        transaction
            .execute(
                "INSERT OR IGNORE INTO catalogue_owners (organization, owner, head) VALUES (?1, ?2, ?3)",
                params![organization, owner, head],
            )
            .map_err(|error| error.to_string())?;
    }
    let fences: BTreeMap<String, bool> = restored
        .fences
        .iter()
        .filter(|fence| {
            nessa_protocol::conversation::domain::ConversationId::new(&fence.id).is_ok()
        })
        .map(|fence| (fence.id.clone(), fence.erased))
        .collect();
    for row in &rows {
        transaction
            .execute(
                "INSERT OR IGNORE INTO conversations (
                    id, organization, owner, creator_surface, creation_action,
                    creation_requested_at_ms, agent, model, approval_mode,
                    creation_revision, change_revision
                 ) VALUES (?1, ?2, ?3, 'restore', 'backup.restore.accept', 0, ?4, ?5, ?6, ?7, ?8)",
                params![
                    row.id,
                    row.organization,
                    row.owner,
                    row.agent,
                    row.model,
                    row.approval_mode,
                    row.creation_revision,
                    row.change_revision
                ],
            )
            .map_err(|error| error.to_string())?;
        if row.record_summary {
            transaction
                .execute(
                    "INSERT OR IGNORE INTO summaries (
                        conversation_id, title, preview, updated_at_ms, archived
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        row.id,
                        row.title,
                        row.preview,
                        row.updated_at_ms,
                        row.archived
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        if let Some(erased) = fences.get(&row.id) {
            let (provider_session, provider_erasure, erased) = if *erased {
                ("absent", Some("no_provider_session"), 1)
            } else {
                ("unread", None, 0)
            };
            transaction
                .execute(
                    "INSERT INTO deletions (
                        conversation_id, organization, initiator, surface, request,
                        requested_at_ms, provider_session, provider_session_id,
                        provider_erasure, erased
                     ) VALUES (?1, ?2, ?3, 'restore', 'backup.restore.accept', 0, ?4, NULL, ?5, ?6)",
                    params![
                        row.id,
                        row.organization,
                        row.owner,
                        provider_session,
                        provider_erasure,
                        erased
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

struct MetadataRow {
    id: String,
    organization: String,
    owner: String,
    agent: String,
    model: String,
    approval_mode: String,
    creation_revision: i64,
    change_revision: i64,
    title: Option<String>,
    preview: Option<String>,
    updated_at_ms: i64,
    archived: i64,
    record_summary: bool,
}

struct RestoredMetadata {
    rows: Vec<MetadataRow>,
    fences: Vec<DeletionFenceRow>,
}

struct DeletionFenceRow {
    id: String,
    erased: bool,
}

struct ConversationFields {
    organization: String,
    owner: String,
    agent: String,
    model: String,
    approval_mode: String,
    creation_revision: i64,
    change_revision: i64,
}

struct SummaryFields {
    title: Option<String>,
    preview: Option<String>,
    updated_at_ms: i64,
    archived: i64,
}

enum Classified {
    Conversation(String),
    Summary(String),
    Tombstone(String),
    Ignore,
}

/// Conversation rows and summary rows are separate export records. A summary
/// names the same conversation and nothing else: its missing owner must not
/// become the conversation, and the conversation's empty title must not hide
/// the saved one. Directory order does not choose.
fn metadata_rows(live: &Path) -> Result<RestoredMetadata, String> {
    let directory = live.join("records").join("conversation-metadata");
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(RestoredMetadata {
            rows: Vec::new(),
            fences: Vec::new(),
        });
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        if entry.path().is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    let mut conversations = BTreeMap::new();
    let mut summaries = BTreeMap::new();
    let mut fences = BTreeMap::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        match classify(name, &value) {
            Classified::Conversation(id) => {
                let Some(fields) = conversation_fields(&value) else {
                    continue;
                };
                conversations.entry(id).or_insert(fields);
            }
            Classified::Summary(id) => {
                summaries.entry(id).or_insert(summary_fields(&value));
            }
            Classified::Tombstone(id) => {
                let erased = value.get("erased").and_then(|item| item.as_bool()) == Some(true);
                fences.entry(id).or_insert(erased);
            }
            Classified::Ignore => {}
        }
    }
    let rows = conversations
        .into_iter()
        .map(|(id, conversation)| {
            let summary = summaries.remove(&id);
            MetadataRow {
                id,
                organization: conversation.organization,
                owner: conversation.owner,
                agent: conversation.agent,
                model: conversation.model,
                approval_mode: conversation.approval_mode,
                creation_revision: conversation.creation_revision,
                change_revision: conversation.change_revision,
                title: summary.as_ref().and_then(|summary| summary.title.clone()),
                preview: summary.as_ref().and_then(|summary| summary.preview.clone()),
                updated_at_ms: summary
                    .as_ref()
                    .map(|summary| summary.updated_at_ms)
                    .unwrap_or(0),
                archived: summary
                    .as_ref()
                    .map(|summary| summary.archived)
                    .unwrap_or(0),
                record_summary: true,
            }
        })
        .collect();
    Ok(RestoredMetadata {
        rows,
        fences: fences
            .into_iter()
            .map(|(id, erased)| DeletionFenceRow { id, erased })
            .collect(),
    })
}

fn classify(file_name: &str, value: &serde_json::Value) -> Classified {
    if let Ok(decoded) = decode_identity(file_name, 512) {
        if let Some(classified) = classified_name(&decoded, value) {
            return classified;
        }
    }
    classified_body(value)
}

fn classified_name(name: &str, value: &serde_json::Value) -> Option<Classified> {
    if let Some(rest) = name.strip_prefix("conversation-") {
        return Some(match text(value, "id") {
            Some(id) if id == rest => Classified::Conversation(id),
            _ => Classified::Ignore,
        });
    }
    if let Some(rest) = name.strip_prefix("summary-") {
        return Some(match text(value, "conversationId") {
            Some(id) if id == rest => Classified::Summary(id),
            _ => Classified::Ignore,
        });
    }
    if let Some(rest) = name.strip_prefix("tombstone-") {
        return Some(match text(value, "conversationId") {
            Some(id) if id == rest => Classified::Tombstone(id),
            _ => Classified::Ignore,
        });
    }
    None
}

fn classified_body(value: &serde_json::Value) -> Classified {
    if value.get("erased").is_some() && value.get("agent").is_none() {
        if let Some(id) = text(value, "conversationId") {
            return Classified::Tombstone(id);
        }
        return Classified::Ignore;
    }
    if value.get("agent").is_some() {
        if let Some(id) = text(value, "id") {
            return Classified::Conversation(id);
        }
    }
    if value.get("id").is_none() {
        if let Some(id) = text(value, "conversationId") {
            return Classified::Summary(id);
        }
    }
    Classified::Ignore
}

fn conversation_fields(value: &serde_json::Value) -> Option<ConversationFields> {
    let creation_revision = positive(value, "creationRevision").unwrap_or(1);
    let change_revision = positive(value, "changeRevision")
        .unwrap_or(creation_revision)
        .max(creation_revision);
    Some(ConversationFields {
        organization: text(value, "organization")?,
        owner: text(value, "owner")?,
        agent: text(value, "agent")?,
        model: text(value, "model").unwrap_or_else(|| "restored".into()),
        approval_mode: match text(value, "approvalMode").as_deref() {
            Some(mode @ ("ask" | "auto" | "full")) => mode.to_owned(),
            _ => "ask".into(),
        },
        creation_revision,
        change_revision,
    })
}

fn summary_fields(value: &serde_json::Value) -> SummaryFields {
    SummaryFields {
        title: text(value, "title"),
        preview: text(value, "preview"),
        updated_at_ms: value
            .get("updatedAtMs")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        archived: i64::from(value.get("archived").and_then(|item| item.as_bool()) == Some(true)),
    }
}

fn positive(value: &serde_json::Value, name: &str) -> Option<i64> {
    value
        .get(name)
        .and_then(|item| item.as_i64())
        .filter(|number| *number > 0)
}

fn text(value: &serde_json::Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(|item| item.as_str())
        .map(str::to_owned)
}

fn merge_stream(serving: &Path, published: &Path, conversation_id: &str) -> Result<(), String> {
    if !serving.exists() {
        publish_copy(published, serving)?;
        return Ok(());
    }
    let published = published
        .to_str()
        .ok_or_else(|| "session path is not Unicode".to_owned())?;
    let mut connection = Connection::open(serving).map_err(|error| error.to_string())?;
    connection
        .execute("ATTACH DATABASE ?1 AS fetched", params![published])
        .map_err(|error| error.to_string())?;
    // The stream, its name, and its records are one conversation. A stop
    // between those inserts used to leave the stream in place, and the next
    // open treated that fragment as the whole history.
    let merged = (|| -> Result<(), String> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let present: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM event_streams WHERE public_id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if present == 0 {
            transaction
                .execute(
                    "INSERT INTO event_streams (public_id, incarnation, floor, tail, retired)
                     SELECT public_id, incarnation, floor, tail, COALESCE(retired, 0)
                     FROM fetched.event_streams WHERE public_id = ?1",
                    params![conversation_id],
                )
                .map_err(|error| error.to_string())?;
            let key: i64 = transaction
                .query_row(
                    "SELECT stream_key FROM event_streams WHERE public_id = ?1",
                    params![conversation_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO event_stream_names (
                        public_id, latest_incarnation, active_stream_key
                     )
                     SELECT public_id, latest_incarnation, ?2
                     FROM fetched.event_stream_names WHERE public_id = ?1",
                    params![conversation_id, key],
                )
                .map_err(|error| error.to_string())?;
            transaction
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
        transaction.commit().map_err(|error| error.to_string())
    })();
    let detached = connection
        .execute("DETACH DATABASE fetched", [])
        .map_err(|error| error.to_string());
    match (merged, detached) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
    }
}
