//! One private SQLite file of remembered permission rules and their audit.
//!
//! A rule change and the audit row that records it commit in the same
//! transaction. A failure rolls both back, so a rule is never stored without
//! its evidence and evidence is never stored for a change that did not
//! commit. Nothing here migrates an older file: a different `user_version`
//! is refused, the same as every other local dataset. This build is version
//! 2. Version 1 stored a provisional column and no audit table; that file is
//! refused rather than read or rewritten.

use nessa_local_database::rusqlite::{params, OptionalExtension};
use nessa_local_database::{rusqlite, OpenError, Schema};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;
use std::path::Path;

use crate::conversation::application::StoredPermissionRule;

const DEFINITION: &str = "
PRAGMA user_version = 2;

CREATE TABLE permission_rule (
    id TEXT PRIMARY KEY NOT NULL,
    revision TEXT NOT NULL,
    workspace TEXT NOT NULL,
    source TEXT NOT NULL,
    tool TEXT NOT NULL,
    effect TEXT NOT NULL,
    granted_principal TEXT NOT NULL,
    granted_surface TEXT NOT NULL,
    granted_request TEXT NOT NULL,
    UNIQUE (workspace, source, tool)
);

CREATE TABLE permission_rule_audit (
    record_id TEXT PRIMARY KEY NOT NULL,
    rule_id TEXT NOT NULL,
    revision TEXT NOT NULL,
    workspace TEXT NOT NULL,
    source TEXT NOT NULL,
    tool TEXT NOT NULL,
    before_effect TEXT NOT NULL,
    after_effect TEXT NOT NULL,
    cause TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    surface_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL
);
";

const IDENTITY: usize = ActionContext::MAX_IDENTITY_BYTES;
const WORKSPACE: usize = 4096;

/// Why a rule was not written. The transaction was not committed.
pub(super) enum WriteError {
    /// SQLite refused the write, or the audit row was refused with it.
    Database,
    /// The row already there has a revision that cannot be bumped.
    Revision,
}

/// One audit row, as the rules file stored it.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StoredRuleAudit {
    pub rule_id: String,
    pub revision: String,
    pub before_effect: String,
    pub after_effect: String,
    pub cause: String,
    pub principal_id: String,
    pub request_id: String,
}

fn failed(cause: rusqlite::Error) -> WriteError {
    tracing::error!(%cause, "permission rule was not written");
    WriteError::Database
}

/// Why the rules could not be read back.
#[derive(Debug)]
pub(super) enum LoadError {
    /// SQLite refused the read.
    Database(rusqlite::Error),
    /// A row in this file is not a rule. It is not skipped and not rewritten.
    NotARule,
}

pub(super) struct RuleStore {
    connection: rusqlite::Connection,
    /// The next committed change fails inside its transaction, so neither
    /// the rule nor the audit row remains. Tests use it.
    #[cfg(test)]
    audit_fails: bool,
}

struct RawRule {
    id: String,
    revision: String,
    workspace: String,
    source: String,
    tool: String,
    effect: String,
    granted_principal: String,
    granted_surface: String,
    granted_request: String,
}

/// One rule save: the key, the effect, who granted it, and when it was observed.
pub(super) struct RememberWrite<'a> {
    pub workspace: &'a str,
    pub source: &'a str,
    pub tool: &'a str,
    pub effect: PermissionEffect,
    pub granted_by: &'a ActionContext,
    pub fresh_id: String,
    pub observed_at_ms: u64,
}

struct AuditWrite<'a> {
    rule_id: &'a str,
    revision: &'a str,
    workspace: &'a str,
    source: &'a str,
    tool: &'a str,
    before_effect: &'a str,
    after_effect: &'a str,
    cause: &'a str,
    actor: &'a ActionContext,
    observed_at_ms: u64,
}

impl RuleStore {
    pub(super) fn open(path: &Path) -> Result<Self, OpenError> {
        let schema = Schema::new(DEFINITION)?;
        let connection = nessa_local_database::open(path, &schema)?;
        Ok(Self {
            connection,
            #[cfg(test)]
            audit_fails: false,
        })
    }

    #[cfg(test)]
    pub(super) fn fail_next_audit(&mut self) {
        self.audit_fails = true;
    }

    /// The rows that are rules, in the order they were stored.
    ///
    /// A row that is not a rule refuses the read. Opening then refuses the
    /// file, and nothing here deletes or rewrites that row.
    pub(super) fn load(&self) -> Result<Vec<StoredPermissionRule>, LoadError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, revision, workspace, source, tool, effect,
                        granted_principal, granted_surface, granted_request
                 FROM permission_rule ORDER BY rowid",
            )
            .map_err(LoadError::Database)?;
        let rows = statement
            .query_map([], read_raw)
            .map_err(LoadError::Database)?;
        let mut rules = Vec::new();
        for row in rows {
            let raw = row.map_err(LoadError::Database)?;
            rules.push(validate(raw).ok_or(LoadError::NotARule)?);
        }
        Ok(rules)
    }

    /// Insert a row this build will not treat as a rule. Tests use it.
    #[cfg(test)]
    pub(super) fn insert_not_a_rule(&mut self) -> Result<(), rusqlite::Error> {
        self.connection.execute(
            "INSERT INTO permission_rule (
                id, revision, workspace, source, tool, effect,
                granted_principal, granted_surface, granted_request
             ) VALUES ('pr_not_a_rule', '1', '/work', 'claude', 'Bash', 'sometimes',
                       'owner', 'surface', 'request')",
            [],
        )?;
        Ok(())
    }

    /// Audit rows in the order they were committed.
    #[cfg(test)]
    pub(super) fn audits(&self) -> Result<Vec<StoredRuleAudit>, rusqlite::Error> {
        let mut statement = self.connection.prepare(
            "SELECT rule_id, revision, before_effect, after_effect, cause,
                    principal_id, request_id
             FROM permission_rule_audit ORDER BY rowid",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(StoredRuleAudit {
                rule_id: row.get(0)?,
                revision: row.get(1)?,
                before_effect: row.get(2)?,
                after_effect: row.get(3)?,
                cause: row.get(4)?,
                principal_id: row.get(5)?,
                request_id: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Insert a rule, or replace the one for this tool in this workspace.
    ///
    /// A replacement keeps the id and bumps the revision. The audit row is
    /// inserted in the same transaction. Nothing is committed when either
    /// write fails.
    pub(super) fn remember(
        &mut self,
        write: RememberWrite<'_>,
    ) -> Result<StoredPermissionRule, WriteError> {
        let RememberWrite {
            workspace,
            source,
            tool,
            effect,
            granted_by,
            fresh_id,
            observed_at_ms,
        } = write;
        let transaction = self.connection.transaction().map_err(failed)?;
        let existing = transaction
            .query_row(
                "SELECT id, revision, workspace, source, tool, effect,
                        granted_principal, granted_surface, granted_request
                 FROM permission_rule WHERE workspace = ?1 AND source = ?2 AND tool = ?3",
                params![workspace, source, tool],
                read_raw,
            )
            .optional()
            .map_err(failed)?;
        let stored = match existing {
            Some(raw) => {
                let Some(current) = validate(raw) else {
                    // A row that is not a rule stays where it is. Replacing it
                    // would erase a history this process cannot read.
                    return Err(WriteError::Database);
                };
                let revision = next_revision(&current.revision).ok_or(WriteError::Revision)?;
                let updated = transaction
                    .execute(
                        "UPDATE permission_rule
                         SET revision = ?1, effect = ?2,
                             granted_principal = ?3, granted_surface = ?4, granted_request = ?5
                         WHERE id = ?6 AND revision = ?7",
                        params![
                            revision,
                            effect_text(effect),
                            granted_by.principal_id(),
                            granted_by.surface_id(),
                            granted_by.request_id(),
                            current.id,
                            current.revision,
                        ],
                    )
                    .map_err(failed)?;
                if updated != 1 {
                    return Err(WriteError::Revision);
                }
                let stored = StoredPermissionRule {
                    id: current.id,
                    revision,
                    workspace: workspace.to_owned(),
                    source: source.to_owned(),
                    tool: tool.to_owned(),
                    effect,
                    granted_by: granted_by.clone(),
                };
                insert_audit(
                    &transaction,
                    AuditWrite {
                        rule_id: &stored.id,
                        revision: &stored.revision,
                        workspace,
                        source,
                        tool,
                        before_effect: effect_text(current.effect),
                        after_effect: effect_text(effect),
                        cause: "replace",
                        actor: granted_by,
                        observed_at_ms,
                    },
                )?;
                stored
            }
            None => {
                transaction
                    .execute(
                        "INSERT INTO permission_rule (
                            id, revision, workspace, source, tool, effect,
                            granted_principal, granted_surface, granted_request
                         ) VALUES (?1, '1', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            fresh_id,
                            workspace,
                            source,
                            tool,
                            effect_text(effect),
                            granted_by.principal_id(),
                            granted_by.surface_id(),
                            granted_by.request_id(),
                        ],
                    )
                    .map_err(failed)?;
                let stored = StoredPermissionRule {
                    id: fresh_id,
                    revision: "1".to_owned(),
                    workspace: workspace.to_owned(),
                    source: source.to_owned(),
                    tool: tool.to_owned(),
                    effect,
                    granted_by: granted_by.clone(),
                };
                insert_audit(
                    &transaction,
                    AuditWrite {
                        rule_id: &stored.id,
                        revision: &stored.revision,
                        workspace,
                        source,
                        tool,
                        before_effect: "absent",
                        after_effect: effect_text(effect),
                        cause: "grant",
                        actor: granted_by,
                        observed_at_ms,
                    },
                )?;
                stored
            }
        };
        #[cfg(test)]
        if self.audit_fails {
            self.audit_fails = false;
            return Err(WriteError::Database);
        }
        transaction.commit().map_err(failed)?;
        Ok(stored)
    }

    /// Delete the rule with `id` and record that it is absent.
    ///
    /// `Ok(false)` when no rule has that id. A row that is not a rule is left
    /// in place: deleting it would erase a history this process cannot record.
    /// A failure commits neither the delete nor the audit row.
    pub(super) fn revoke(
        &mut self,
        id: &str,
        actor: &ActionContext,
        observed_at_ms: u64,
    ) -> Result<bool, WriteError> {
        let transaction = self.connection.transaction().map_err(failed)?;
        let existing = transaction
            .query_row(
                "SELECT id, revision, workspace, source, tool, effect,
                        granted_principal, granted_surface, granted_request
                 FROM permission_rule WHERE id = ?1",
                params![id],
                read_raw,
            )
            .optional()
            .map_err(failed)?;
        let Some(raw) = existing else {
            return Ok(false);
        };
        let Some(rule) = validate(raw) else {
            return Err(WriteError::Database);
        };
        let deleted = transaction
            .execute("DELETE FROM permission_rule WHERE id = ?1", params![id])
            .map_err(failed)?;
        if deleted == 0 {
            return Ok(false);
        }
        insert_audit(
            &transaction,
            AuditWrite {
                rule_id: &rule.id,
                revision: &rule.revision,
                workspace: &rule.workspace,
                source: &rule.source,
                tool: &rule.tool,
                before_effect: effect_text(rule.effect),
                after_effect: "absent",
                cause: "revoke",
                actor,
                observed_at_ms,
            },
        )?;
        #[cfg(test)]
        if self.audit_fails {
            self.audit_fails = false;
            return Err(WriteError::Database);
        }
        transaction.commit().map_err(failed)?;
        Ok(true)
    }
}

fn insert_audit(
    transaction: &rusqlite::Transaction<'_>,
    audit: AuditWrite<'_>,
) -> Result<(), WriteError> {
    let record_id = format!("{}:{}:{}", audit.cause, audit.rule_id, audit.revision);
    transaction
        .execute(
            "INSERT INTO permission_rule_audit (
                record_id, rule_id, revision, workspace, source, tool,
                before_effect, after_effect, cause,
                principal_id, surface_id, request_id, observed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                record_id,
                audit.rule_id,
                audit.revision,
                audit.workspace,
                audit.source,
                audit.tool,
                audit.before_effect,
                audit.after_effect,
                audit.cause,
                audit.actor.principal_id(),
                audit.actor.surface_id(),
                audit.actor.request_id(),
                audit.observed_at_ms as i64,
            ],
        )
        .map_err(failed)?;
    Ok(())
}

fn read_raw(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRule> {
    Ok(RawRule {
        id: row.get(0)?,
        revision: row.get(1)?,
        workspace: row.get(2)?,
        source: row.get(3)?,
        tool: row.get(4)?,
        effect: row.get(5)?,
        granted_principal: row.get(6)?,
        granted_surface: row.get(7)?,
        granted_request: row.get(8)?,
    })
}

fn validate(raw: RawRule) -> Option<StoredPermissionRule> {
    if !kept(&raw.id, IDENTITY)
        || !kept(&raw.revision, IDENTITY)
        || !kept(&raw.workspace, WORKSPACE)
        || !kept(&raw.tool, IDENTITY)
        || (raw.source != "claude" && raw.source != "codex")
    {
        return None;
    }
    let effect = match raw.effect.as_str() {
        "allow" => PermissionEffect::Allow,
        "deny" => PermissionEffect::Deny,
        _ => return None,
    };
    let granted_by = ActionContext::new(
        raw.granted_principal,
        raw.granted_surface,
        raw.granted_request,
    )
    .ok()?;
    Some(StoredPermissionRule {
        id: raw.id,
        revision: raw.revision,
        workspace: raw.workspace,
        source: raw.source,
        tool: raw.tool,
        effect,
        granted_by,
    })
}

fn kept(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max
}

fn effect_text(effect: PermissionEffect) -> &'static str {
    match effect {
        PermissionEffect::Allow => "allow",
        PermissionEffect::Deny => "deny",
    }
}

fn next_revision(current: &str) -> Option<String> {
    let revision = current.parse::<u64>().ok()?;
    Some(revision.checked_add(1)?.to_string())
}
