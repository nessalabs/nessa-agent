//! The grant file is opened and version-checked before the conversation file.
//! The existing SQL adapter attaches it as `grants`; explicit qualifiers keep
//! old metadata grant tables outside the read and write paths. An attached
//! transaction retains atomic grant evidence and catalogue publication.
use nessa_local_database::{rusqlite::Connection, OpenError, Schema, Upgrade};
use std::path::{Path, PathBuf};

/// Production grants are identity/access data, independent of conversation
/// presence and listener configuration. Retirement inspects `conversations/`.
pub fn access_grants_path(namespace: &Path) -> PathBuf {
    namespace
        .join("access-grants")
        .join("access-grants.sqlite3")
}

pub(super) fn path(metadata: &Path) -> PathBuf {
    metadata.with_extension("grants.sqlite3")
}

/// The steps that bring an older grant file to [`legacy_schema`]'s version. Version 1
/// is the grant rows without the log. The step adds the empty log tables and
/// leaves every grant row where it was.
static UPGRADES: [Upgrade; 1] = [Upgrade::new(1, include_str!("grants_from_1.sql"))];

fn legacy_schema() -> Result<Schema, OpenError> {
    Ok(Schema::new(include_str!("grants.sql"))?
        .with_upgrades(&UPGRADES)?
        .with_validation(|connection| {
            validate_shape(connection, "", GrantFormat::ConversationOnly)
        }))
}

// This definition is recognition only. The live opener selects it only
// after a typed version-3 refusal; it never registers a version-2 upgrade.
fn activated_schema() -> Result<Schema, OpenError> {
    Ok(Schema::new(concat!(
        include_str!("grants.sql"),
        "\n",
        include_str!("grants_from_2.sql")
    ))?
    .with_validation(|connection| validate_shape(connection, "", GrantFormat::Directory)))
}

#[derive(Clone, Copy)]
enum GrantFormat {
    ConversationOnly,
    Directory,
}

pub(super) fn open(path: &Path) -> Result<Connection, OpenError> {
    match nessa_local_database::open(path, &legacy_schema()?) {
        // Only the exact newer known format gets another look. No create,
        // migration, or generic fallback on corruption or I/O failure.
        Err(OpenError::Version { found: 3, .. }) => {
            nessa_local_database::open_existing(path, &activated_schema()?)
        }
        result => result,
    }
}

pub(super) fn attach(
    connection: &Connection,
    path: &Path,
) -> nessa_local_database::rusqlite::Result<()> {
    connection.execute(
        "ATTACH DATABASE ?1 AS grants",
        [path
            .to_str()
            .ok_or(nessa_local_database::rusqlite::Error::InvalidPath(
                path.to_owned(),
            ))?],
    )?;
    let db = Some("grants");
    connection.pragma_update(db, "secure_delete", true)?;
    connection.pragma_update(db, "synchronous", "FULL")?;
    connection.pragma_update_and_check(db, "journal_mode", "DELETE", |_| Ok(()))?;
    Ok(())
}

/// Snapshots attach without write permission and enforce the same schema
/// version before any grant query. A missing file is not created.
pub(super) fn attach_read_only(connection: &Connection, path: &Path) -> Result<(), OpenError> {
    let mut uri = url::Url::from_file_path(path).map_err(|_| {
        OpenError::File(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "grant snapshot requires an absolute path",
        ))
    })?;
    uri.set_query(Some("mode=ro"));
    connection.execute("ATTACH DATABASE ?1 AS grants", [uri.as_str()])?;
    let found = connection
        .pragma_query_value(Some("grants"), "user_version", |row| row.get::<_, i64>(0))?;
    let format = match found {
        2 => GrantFormat::ConversationOnly,
        3 => GrantFormat::Directory,
        _ => {
            return Err(OpenError::Version {
                found,
                expected: legacy_schema()?.version(),
            })
        }
    };
    validate_shape(connection, "grants.", format)
}

/// Identify the refused file without directing recovery at another dataset.
#[derive(Debug)]
pub enum ConversationStoreOpenError {
    /// The conversation metadata opener refused its own file.
    Metadata(OpenError),
    /// The grant opener refused before the metadata file was opened.
    Grants { path: PathBuf, cause: OpenError },
}
impl From<OpenError> for ConversationStoreOpenError {
    fn from(cause: OpenError) -> Self {
        Self::Metadata(cause)
    }
}
impl std::fmt::Display for ConversationStoreOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata(cause) => write!(f, "conversation metadata: {cause}"),
            Self::Grants { path, cause } => {
                write!(f, "access grants at {}: {cause}", path.display())
            }
        }
    }
}
impl std::error::Error for ConversationStoreOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Metadata(cause) | Self::Grants { cause, .. } => cause,
        })
    }
}

/// One owner of the current grant columns, for live and read-only openers.
fn validate_shape(
    connection: &Connection,
    database: &str,
    format: GrantFormat,
) -> Result<(), OpenError> {
    let ordinary = [
        ("share_grants","subject_kind, subject_id, principal_id, resource_kind, resource_id, receiver_id, credential_id, capabilities, policy, policy_revision, granted_at_ms"),
        ("share_grant_changes","sequence, principal_id, resource_kind, resource_id, receiver_id, credential_id, before, after, policy_before, policy_after, initiator, surface, request, changed_at_ms, revision"),
        ("person_links","organization_id, gateway_principal, person_principal, credential_id, invitation"),
        ("person_link_changes","sequence, organization_id, gateway_principal, credential_id, invitation, person_before, person_after, cause, initiator_kind, initiator_id, observed_at_ms"),
        ("grant_streams","receiver_id, incarnation, head"),
        ("grant_log","receiver_id, position, conversation_id, capabilities, policy, policy_revision"),
    ];
    let directory: &[(&str, &str)] = match format {
        GrantFormat::ConversationOnly => &[],
        GrantFormat::Directory => &[
            ("directory_grant_state","resource_kind, environment, canonical_path, subject_kind, subject_id, capabilities, rules, revision"),
            ("directory_receiver_log","receiver_id, position, resource_kind, environment, canonical_path, capabilities, revision"),
            ("directory_grant_evidence","sequence, evidence_id, initiator, request_id, event_kind, environment, canonical_path, subject_kind, subject_id, input, outcome, delivered"),
        ],
    };
    for (table, columns) in ordinary.iter().chain(directory) {
        connection
            .prepare(&format!("SELECT {columns} FROM {database}{table} LIMIT 0"))
            .map_err(|cause| OpenError::Damaged(format!("grant table {table}: {cause}")))?;
    }
    Ok(())
}
