//! The grant file is opened and version-checked before the conversation file.
//! The existing SQL adapter attaches it as `grants`; explicit qualifiers keep
//! old metadata grant tables outside the read and write paths. An attached
//! transaction retains atomic grant evidence and catalogue publication.
use nessa_local_database::{rusqlite::Connection, OpenError, Schema};
use std::path::{Path, PathBuf};

pub(super) fn path(metadata: &Path) -> PathBuf {
    metadata.with_extension("grants.sqlite3")
}

pub(super) fn open(path: &Path) -> Result<Connection, OpenError> {
    let connection = nessa_local_database::open(path, &Schema::new(include_str!("grants.sql"))?)?;
    validate_shape(&connection, "")?;
    Ok(connection)
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
    let expected = Schema::new(include_str!("grants.sql"))?.version();
    if found != i64::from(expected) {
        return Err(OpenError::Version { found, expected });
    }
    validate_shape(connection, "grants.")
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
fn validate_shape(connection: &Connection, database: &str) -> Result<(), OpenError> {
    for (table,columns) in [
        ("share_grants","subject_kind, subject_id, principal_id, resource_kind, resource_id, receiver_id, credential_id, capabilities, policy, policy_revision, granted_at_ms"),
        ("share_grant_changes","sequence, principal_id, resource_kind, resource_id, receiver_id, credential_id, before, after, policy_before, policy_after, initiator, surface, request, changed_at_ms, revision"),
        ("person_links","organization_id, gateway_principal, person_principal, credential_id, invitation"),
        ("person_link_changes","sequence, organization_id, gateway_principal, credential_id, invitation, person_before, person_after, cause, initiator_kind, initiator_id, observed_at_ms"),
    ] {
        connection.prepare(&format!("SELECT {columns} FROM {database}{table} LIMIT 0")).map_err(|cause| OpenError::Damaged(format!("grant table {table}: {cause}")))?;
    }
    Ok(())
}
