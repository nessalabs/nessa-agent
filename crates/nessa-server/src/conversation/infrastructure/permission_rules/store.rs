//! One private SQLite file of remembered permission rules.
//!
//! A row that cannot be read as a rule is skipped. The file still opens, and
//! the other rows still answer reviews. Nothing here migrates an older file:
//! a different `user_version` is refused, the same as every other local dataset.

use nessa_local_database::rusqlite::{params, OptionalExtension};
use nessa_local_database::{rusqlite, OpenError, Schema};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;
use std::path::Path;

use crate::conversation::application::StoredPermissionRule;

const DEFINITION: &str = "
PRAGMA user_version = 1;

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
";

const IDENTITY: usize = ActionContext::MAX_IDENTITY_BYTES;
const WORKSPACE: usize = 4096;

/// Why a rule was not written.
pub(super) enum WriteError {
    /// SQLite refused the write. Nothing was committed.
    Database,
    /// The row already there has a revision that cannot be bumped.
    Revision,
}

fn failed(cause: rusqlite::Error) -> WriteError {
    tracing::error!(%cause, "permission rule was not written");
    WriteError::Database
}

pub(super) struct RuleStore {
    connection: rusqlite::Connection,
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

impl RuleStore {
    pub(super) fn open(path: &Path) -> Result<Self, OpenError> {
        let schema = Schema::new(DEFINITION)?;
        let connection = nessa_local_database::open(path, &schema)?;
        Ok(Self { connection })
    }

    /// The rows that are rules, in the order they were stored.
    pub(super) fn load(&self) -> Result<Vec<StoredPermissionRule>, rusqlite::Error> {
        let mut statement = self.connection.prepare(
            "SELECT id, revision, workspace, source, tool, effect,
                    granted_principal, granted_surface, granted_request
             FROM permission_rule ORDER BY rowid",
        )?;
        let rows = statement.query_map([], read_raw)?;
        let mut rules = Vec::new();
        for row in rows {
            if let Ok(Some(rule)) = row.map(validate) {
                rules.push(rule);
            }
        }
        Ok(rules)
    }

    /// Insert a rule, or replace the one for this tool in this workspace.
    /// A replacement keeps the id and bumps the revision. An unreadable row
    /// occupying the key is deleted and a new rule takes its place.
    pub(super) fn remember(
        &mut self,
        workspace: &str,
        source: &str,
        tool: &str,
        effect: PermissionEffect,
        granted_by: &ActionContext,
        fresh_id: String,
    ) -> Result<StoredPermissionRule, WriteError> {
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
        let stored = match existing.and_then(validate) {
            Some(current) => {
                let revision = next_revision(&current.revision).ok_or(WriteError::Revision)?;
                transaction
                    .execute(
                        "UPDATE permission_rule
                         SET revision = ?1, effect = ?2,
                             granted_principal = ?3, granted_surface = ?4, granted_request = ?5
                         WHERE id = ?6",
                        params![
                            revision,
                            effect_text(effect),
                            granted_by.principal_id(),
                            granted_by.surface_id(),
                            granted_by.request_id(),
                            current.id,
                        ],
                    )
                    .map_err(failed)?;
                StoredPermissionRule {
                    id: current.id,
                    revision,
                    workspace: workspace.to_owned(),
                    source: source.to_owned(),
                    tool: tool.to_owned(),
                    effect,
                    granted_by: granted_by.clone(),
                }
            }
            None => {
                transaction
                    .execute(
                        "DELETE FROM permission_rule
                         WHERE workspace = ?1 AND source = ?2 AND tool = ?3",
                        params![workspace, source, tool],
                    )
                    .map_err(failed)?;
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
                StoredPermissionRule {
                    id: fresh_id,
                    revision: "1".to_owned(),
                    workspace: workspace.to_owned(),
                    source: source.to_owned(),
                    tool: tool.to_owned(),
                    effect,
                    granted_by: granted_by.clone(),
                }
            }
        };
        transaction.commit().map_err(failed)?;
        Ok(stored)
    }

    /// Delete the rule with `id`. `false` when there is none.
    pub(super) fn revoke(&mut self, id: &str) -> Result<bool, rusqlite::Error> {
        Ok(self
            .connection
            .execute("DELETE FROM permission_rule WHERE id = ?1", params![id])?
            > 0)
    }
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

#[cfg(test)]
pub(super) fn schema() -> Schema {
    Schema::new(DEFINITION).expect("permission rule schema states one version")
}

fn next_revision(current: &str) -> Option<String> {
    let revision = current.parse::<u64>().ok()?;
    Some(revision.checked_add(1)?.to_string())
}
