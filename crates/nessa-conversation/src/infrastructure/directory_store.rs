//! Directory resources and grants in the grant database.

use std::path::Path;
use std::sync::Mutex;

use nessa_local_database::rusqlite::{params, OptionalExtension};
use nessa_local_database::{OpenError, Schema};

use crate::application::{
    DirectoryAuditRecord, DirectoryRefusal, DirectoryStore, PendingAudit, PendingCleanup,
    PendingOpenAudit, RevokeMismatch, StoredDirectoryGrant, StoredDirectoryResource,
};
use crate::domain::{ArgvPattern, CommandMatcher, CommandRules, RuleEffect, BINDING_LEN};
use crate::timer::clock::wall_now;

/// SQLite directory grants. A version other than this build's is refused and not rewritten.
pub struct SqliteDirectoryStore {
    connection: Mutex<nessa_local_database::rusqlite::Connection>,
}

impl SqliteDirectoryStore {
    /// Open `path` with the grant schema.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection =
            nessa_local_database::open(path, &Schema::new(include_str!("store/grants.sql"))?)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

impl DirectoryStore for SqliteDirectoryStore {
    fn insert_resource(&self, resource: &StoredDirectoryResource) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "INSERT INTO directory_resources (resource_id, environment, canonical_path, binding) VALUES (?1, ?2, ?3, ?4)",
                params![resource.resource_id, resource.environment, resource.canonical_path, resource.binding.to_vec()],
            )
            .map_err(store)?;
        Ok(())
    }

    fn resource_at(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .query_row(
                "SELECT resource_id, environment, canonical_path, binding FROM directory_resources WHERE environment = ?1 AND canonical_path = ?2",
                params![environment, canonical],
                row_resource,
            )
            .optional()
            .map_err(store)
    }

    fn resource(
        &self,
        resource_id: &str,
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .query_row(
                "SELECT resource_id, environment, canonical_path, binding FROM directory_resources WHERE resource_id = ?1",
                params![resource_id],
                row_resource,
            )
            .optional()
            .map_err(store)
    }

    fn resource_by_binding(
        &self,
        binding: &[u8; BINDING_LEN],
    ) -> Result<Option<StoredDirectoryResource>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .query_row(
                "SELECT resource_id, environment, canonical_path, binding FROM directory_resources WHERE binding = ?1",
                params![binding.to_vec()],
                row_resource,
            )
            .optional()
            .map_err(store)
    }

    fn insert_grant(
        &self,
        grant: &StoredDirectoryGrant,
        initiator: &str,
    ) -> Result<(), DirectoryRefusal> {
        let mut connection = self.connection.lock().expect("directory grants");
        let principal = grant.principal_id.clone().unwrap_or_else(|| {
            format!("receiver:{}", grant.receiver_id.clone().unwrap_or_default())
        });
        let policy = encode_rules(&grant.rules);
        let now = now_ms();
        let credential = format!("{}:{principal}", grant.resource_id);
        let request = format!("share:{credential}:{now}");
        let transaction = connection.transaction().map_err(store)?;
        transaction
            .execute(
                "INSERT INTO share_grant_changes (
                    principal_id, resource_kind, resource_id, receiver_id, credential_id,
                    before, after, policy_before, policy_after, initiator, surface, request,
                    changed_at_ms, revision
                ) VALUES (?1, 'directory', ?2, ?3, ?4, 'none', ?5, NULL, ?6, ?7, 'directory', ?8, ?9, 0)",
                params![
                    principal,
                    grant.resource_id,
                    grant.receiver_id,
                    credential,
                    grant.capabilities,
                    policy,
                    initiator,
                    request,
                    now,
                ],
            )
            .map_err(store)?;
        let revision = transaction.last_insert_rowid();
        transaction
            .execute(
                "INSERT INTO share_grants (
                    principal_id, resource_kind, resource_id, receiver_id, credential_id,
                    capabilities, policy, policy_revision, granted_at_ms
                ) VALUES (?1, 'directory', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    principal,
                    grant.resource_id,
                    grant.receiver_id,
                    format!("{}:{principal}", grant.resource_id),
                    grant.capabilities,
                    policy,
                    revision,
                    now,
                ],
            )
            .map_err(store)?;
        transaction.commit().map_err(store)?;
        Ok(())
    }

    fn grants_for_subject(
        &self,
        subject: &str,
    ) -> Result<Vec<StoredDirectoryGrant>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        let mut statement = connection
            .prepare(
                "SELECT resource_id, principal_id, receiver_id, capabilities, policy
                 FROM share_grants
                 WHERE resource_kind = 'directory'
                   AND (principal_id = ?1 OR receiver_id = ?1 OR resource_id = ?1)",
            )
            .map_err(store)?;
        let rows = statement
            .query_map(params![subject], row_grant)
            .map_err(store)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store)
    }

    fn grants_for_resource(
        &self,
        resource_id: &str,
    ) -> Result<Vec<StoredDirectoryGrant>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        let mut statement = connection
            .prepare(
                "SELECT resource_id, principal_id, receiver_id, capabilities, policy
                 FROM share_grants WHERE resource_kind = 'directory' AND resource_id = ?1",
            )
            .map_err(store)?;
        let rows = statement
            .query_map(params![resource_id], row_grant)
            .map_err(store)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store)
    }

    fn revoke_mismatch(
        &self,
        resource_id: &str,
        cause: &str,
        initiator: &str,
    ) -> Result<RevokeMismatch, DirectoryRefusal> {
        let mut connection = self.connection.lock().expect("directory grants");
        let transaction = connection.transaction().map_err(store)?;
        let grants = {
            let mut statement = transaction
                .prepare(
                    "SELECT principal_id, receiver_id, credential_id, capabilities, policy, policy_revision
                     FROM share_grants WHERE resource_kind = 'directory' AND resource_id = ?1",
                )
                .map_err(store)?;
            let rows = statement
                .query_map(params![resource_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(store)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(store)?
        };
        if grants.is_empty() {
            if let Some((stored_cause, stored_initiator)) = transaction
                .query_row(
                    "SELECT cause, initiator FROM directory_open_audits WHERE resource_id = ?1",
                    params![resource_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(store)?
            {
                transaction.commit().map_err(store)?;
                return Ok(RevokeMismatch {
                    cause: stored_cause,
                    initiator: stored_initiator,
                    removed_now: false,
                });
            }
            transaction
                .execute(
                    "INSERT INTO directory_open_audits (resource_id, cause, initiator, audit_done) VALUES (?1, ?2, ?3, 0)",
                    params![resource_id, cause, initiator],
                )
                .map_err(store)?;
            transaction.commit().map_err(store)?;
            return Ok(RevokeMismatch {
                cause: cause.to_owned(),
                initiator: initiator.to_owned(),
                removed_now: true,
            });
        }
        if let Some((stored_cause, stored_initiator)) = transaction
            .query_row(
                "SELECT cause, initiator FROM directory_open_audits WHERE resource_id = ?1 AND audit_done = 0",
                params![resource_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(store)?
        {
            let (environment, canonical) = transaction
                .query_row(
                    "SELECT environment, canonical_path FROM directory_resources WHERE resource_id = ?1",
                    params![resource_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(store)?
                .unwrap_or_default();
            transaction
                .execute(
                    "INSERT INTO directory_pending_audits (
                        resource_id, cause, initiator, environment, canonical_path, profile, before, after, at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'active', 'absent', ?6)",
                    params![resource_id, stored_cause, stored_initiator, environment, canonical, now_ms()],
                )
                .map_err(store)?;
        }
        let now = now_ms();
        for (principal, receiver, credential, capabilities, policy, revision) in grants {
            transaction
                .execute(
                    "INSERT INTO share_grant_changes (
                        principal_id, resource_kind, resource_id, receiver_id, credential_id,
                        before, after, policy_before, policy_after, initiator, surface, request,
                        changed_at_ms, revision
                    ) VALUES (?1, 'directory', ?2, ?3, ?4, ?5, 'none', ?6, NULL, ?7, 'directory', ?8, ?9, 0)",
                    params![
                        principal,
                        resource_id,
                        receiver,
                        credential,
                        capabilities,
                        policy,
                        initiator,
                        format!("revoke:{credential}:{revision}"),
                        now,
                    ],
                )
                .map_err(store)?;
        }
        transaction
            .execute(
                "DELETE FROM share_grants WHERE resource_kind = 'directory' AND resource_id = ?1",
                params![resource_id],
            )
            .map_err(store)?;
        transaction
            .execute(
                "INSERT INTO directory_open_audits (resource_id, cause, initiator, audit_done)
                 VALUES (?1, ?2, ?3, 0)
                 ON CONFLICT(resource_id) DO UPDATE SET
                    cause = excluded.cause,
                    initiator = excluded.initiator,
                    audit_done = 0",
                params![resource_id, cause, initiator],
            )
            .map_err(store)?;
        transaction.commit().map_err(store)?;
        Ok(RevokeMismatch {
            cause: cause.to_owned(),
            initiator: initiator.to_owned(),
            removed_now: true,
        })
    }

    fn mark_open_audit(&self, resource_id: &str) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "UPDATE directory_open_audits SET audit_done = 1 WHERE resource_id = ?1",
                params![resource_id],
            )
            .map_err(store)?;
        Ok(())
    }

    fn open_audits_pending(&self) -> Result<Vec<String>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        let mut statement = connection
            .prepare("SELECT resource_id FROM directory_open_audits WHERE audit_done = 0")
            .map_err(store)?;
        let rows = statement.query_map([], |row| row.get(0)).map_err(store)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store)
    }

    fn pending_resources(&self) -> Result<Vec<String>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        let mut statement = connection
            .prepare("SELECT DISTINCT resource_id FROM directory_pending_audits")
            .map_err(store)?;
        let rows = statement.query_map([], |row| row.get(0)).map_err(store)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store)
    }

    fn pending_open_audit(
        &self,
        resource_id: &str,
    ) -> Result<Option<PendingOpenAudit>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .query_row(
                "SELECT cause, initiator FROM directory_open_audits WHERE resource_id = ?1 AND audit_done = 0",
                params![resource_id],
                |row| {
                    Ok(PendingOpenAudit {
                        cause: row.get(0)?,
                        initiator: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(store)
    }

    fn record_cleanup(
        &self,
        environment: &str,
        canonical: &str,
        binding: &[u8; BINDING_LEN],
        initiator: &str,
    ) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "INSERT INTO directory_cleanup (environment, canonical_path, binding, unlinked, audit_done, initiator)
                 VALUES (?1, ?2, ?3, 0, 0, ?4)",
                params![environment, canonical, binding.to_vec(), initiator],
            )
            .map_err(store)?;
        Ok(())
    }

    fn pending_cleanups(&self) -> Result<Vec<(String, String)>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        let mut statement = connection
            .prepare("SELECT environment, canonical_path FROM directory_cleanup")
            .map_err(store)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(store)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store)
    }

    fn cleanup(
        &self,
        environment: &str,
        canonical: &str,
    ) -> Result<Option<PendingCleanup>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .query_row(
                "SELECT binding, unlinked, audit_done, initiator FROM directory_cleanup WHERE environment = ?1 AND canonical_path = ?2",
                params![environment, canonical],
                |row| {
                    let bytes: Vec<u8> = row.get(0)?;
                    let mut binding = [0; BINDING_LEN];
                    binding.copy_from_slice(&bytes);
                    Ok(PendingCleanup {
                        binding,
                        unlinked: row.get::<_, i64>(1)? == 1,
                        audit_done: row.get::<_, i64>(2)? == 1,
                        initiator: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(store)
    }

    fn update_cleanup(
        &self,
        environment: &str,
        canonical: &str,
        unlinked: bool,
        audit_done: bool,
    ) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "UPDATE directory_cleanup SET unlinked = ?3, audit_done = ?4 WHERE environment = ?1 AND canonical_path = ?2",
                params![environment, canonical, unlinked as i64, audit_done as i64],
            )
            .map_err(store)?;
        Ok(())
    }

    fn clear_cleanup(&self, environment: &str, canonical: &str) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "DELETE FROM directory_cleanup WHERE environment = ?1 AND canonical_path = ?2",
                params![environment, canonical],
            )
            .map_err(store)?;
        Ok(())
    }

    fn save_pending(&self, record: &DirectoryAuditRecord) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "INSERT INTO directory_pending_audits (
                    resource_id, cause, initiator, environment, canonical_path, profile, before, after, at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.resource_id.clone().unwrap_or_default(),
                    record.cause,
                    record.initiator,
                    record.environment,
                    record.canonical_path,
                    record.profile,
                    record.before,
                    record.after,
                    i64::try_from(record.at_ms).unwrap_or(i64::MAX),
                ],
            )
            .map_err(store)?;
        Ok(())
    }

    fn pending_for(&self, resource_id: &str) -> Result<Vec<PendingAudit>, DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        let mut statement = connection
            .prepare(
                "SELECT sequence, resource_id, cause, initiator, environment, canonical_path, profile, before, after, at_ms
                 FROM directory_pending_audits WHERE resource_id = ?1 ORDER BY sequence",
            )
            .map_err(store)?;
        let rows = statement
            .query_map(params![resource_id], |row| {
                Ok(PendingAudit {
                    sequence: row.get(0)?,
                    record: DirectoryAuditRecord {
                        cause: row.get(2)?,
                        resource_id: Some(row.get(1)?),
                        environment: row.get(4)?,
                        canonical_path: row.get(5)?,
                        initiator: row.get(3)?,
                        before: row.get(7)?,
                        after: row.get(8)?,
                        profile: row.get(6)?,
                        at_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
                    },
                })
            })
            .map_err(store)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(store)
    }

    fn drop_pending(&self, sequence: i64) -> Result<(), DirectoryRefusal> {
        let connection = self.connection.lock().expect("directory grants");
        connection
            .execute(
                "DELETE FROM directory_pending_audits WHERE sequence = ?1",
                params![sequence],
            )
            .map_err(store)?;
        Ok(())
    }
}

fn row_resource(
    row: &nessa_local_database::rusqlite::Row<'_>,
) -> nessa_local_database::rusqlite::Result<StoredDirectoryResource> {
    let bytes: Vec<u8> = row.get(3)?;
    let mut binding = [0; BINDING_LEN];
    if bytes.len() != BINDING_LEN {
        return Err(nessa_local_database::rusqlite::Error::InvalidQuery);
    }
    binding.copy_from_slice(&bytes);
    Ok(StoredDirectoryResource {
        resource_id: row.get(0)?,
        environment: row.get(1)?,
        canonical_path: row.get(2)?,
        binding,
    })
}

fn row_grant(
    row: &nessa_local_database::rusqlite::Row<'_>,
) -> nessa_local_database::rusqlite::Result<StoredDirectoryGrant> {
    let principal: String = row.get(1)?;
    let receiver: Option<String> = row.get(2)?;
    Ok(StoredDirectoryGrant {
        resource_id: row.get(0)?,
        principal_id: if principal.starts_with("receiver:") {
            None
        } else {
            Some(principal)
        },
        receiver_id: receiver,
        capabilities: row.get(3)?,
        rules: decode_rules(row.get(4)?),
    })
}

fn encode_rules(rules: &CommandRules) -> Option<String> {
    if !rules.constrains_commands() {
        return None;
    }
    let mut text = String::from("[");
    for (index, rule) in rules.rules().iter().enumerate() {
        if index > 0 {
            text.push(',');
        }
        let effect = match rule.effect() {
            RuleEffect::Allow => "allow",
            RuleEffect::Deny => "deny",
        };
        text.push_str("{\"effect\":\"");
        text.push_str(effect);
        text.push_str("\",\"program\":");
        text.push_str(&quote(rule.program()));
        if let Some(argv) = rule.argv() {
            text.push_str(",\"argv\":");
            text.push_str(&quote(argv.as_str()));
        }
        text.push('}');
    }
    text.push(']');
    Some(text)
}

fn decode_rules(text: Option<String>) -> CommandRules {
    let Some(text) = text else {
        return CommandRules::none();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return CommandRules::unreadable();
    };
    let Some(items) = value.as_array() else {
        return CommandRules::unreadable();
    };
    let mut rules = Vec::new();
    for item in items {
        let effect = match item.get("effect").and_then(|value| value.as_str()) {
            Some("allow") => RuleEffect::Allow,
            Some("deny") => RuleEffect::Deny,
            _ => return CommandRules::unreadable(),
        };
        let Some(program) = item.get("program").and_then(|value| value.as_str()) else {
            return CommandRules::unreadable();
        };
        let argv = match item.get("argv").and_then(|value| value.as_str()) {
            Some(text) => match ArgvPattern::new(text) {
                Ok(pattern) => Some(pattern),
                Err(_) => return CommandRules::unreadable(),
            },
            None => None,
        };
        let Ok(matcher) = CommandMatcher::new(effect, program, argv) else {
            return CommandRules::unreadable();
        };
        rules.push(matcher);
    }
    CommandRules::new(rules).unwrap_or_else(|_| CommandRules::unreadable())
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

fn store(error: nessa_local_database::rusqlite::Error) -> DirectoryRefusal {
    tracing::debug!(%error, "directory grant store");
    DirectoryRefusal::Store
}

fn now_ms() -> i64 {
    wall_now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
