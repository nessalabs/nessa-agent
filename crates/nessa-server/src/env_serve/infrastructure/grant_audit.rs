//! Every change this host's owner makes to what a gateway is granted, kept
//! for good: `<data>/environment/grants-audit.jsonl`, one JSON line per
//! change, beside the grants it audits.
//!
//! ```text
//! {"schemaVersion": 1, "committedAtMs": 1760000000000, "initiator": "unix:1000",
//!  "peer": "ssh", "operation": {"op": "grant", "agents": ["claude"]},
//!  "before": {"agents": "*", "commands": {"maxTimeoutMs": 3600000}},
//!  "after": {...}, "outcome": "changed"}
//! ```
//!
//! ```text
//! FileGrantAudit::record ──▶ open (private, appending) ──▶ the line written
//!   ──▶ synced ──▶ Ok, and only then grants.json is renamed in place
//! ```
//!
//! Arrows are steps, in order. It is written while the grants' lock is
//! held, so records are in the order the changes were made. `before` and
//! `after` are the peer's whole grant, its defaults included; `outcome` is
//! `changed`, `unchanged`, or `not_written` for a change whose record was
//! kept but whose grants could not then be written. Lines are only ever
//! appended, never rewritten. The journal is not bounded: it grows by one
//! line, at most about two grants entries long, for each change the owner
//! makes by hand, which no gateway can cause.
use super::grants::{grant_value, peer_name};
use crate::env_serve::{
    application::{GrantAudit, GrantChange, GrantOutcome},
    domain::{Agents, GrantOperation},
};
use nessa_local_storage::{open, OpenMode};
use serde_json::{json, Value};
use std::{
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// The journal, beneath `<data>/environment`.
const FILE: &str = "grants-audit.jsonl";
/// The shape of each line.
const SCHEMA_VERSION: u32 = 1;

/// The grants' audit journal of one data directory.
pub(crate) struct FileGrantAudit {
    path: PathBuf,
}

impl FileGrantAudit {
    /// The journal kept in `environment`, `<data>/environment`.
    pub(crate) fn new(environment: &Path) -> Self {
        Self {
            path: environment.join(FILE),
        }
    }
}

impl GrantAudit for FileGrantAudit {
    fn record(&self, change: &GrantChange) -> Result<(), String> {
        let mut line = serde_json::to_vec(&line(change)).map_err(|error| error.to_string())?;
        line.push(b'\n');
        let failed = |error: std::io::Error| format!("{FILE} could not be written: {error}");
        let mut file = open(&self.path, OpenMode::OpenOrCreate).map_err(failed)?;
        file.seek(SeekFrom::End(0)).map_err(failed)?;
        file.write_all(&line).map_err(failed)?;
        file.sync_data().map_err(failed)
    }
}

fn line(change: &GrantChange) -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "committedAtMs": change.committed_at_ms,
        "initiator": change.initiator,
        "peer": peer_name(&change.peer),
        "operation": operation(&change.operation),
        "before": grant_value(&change.before),
        "after": grant_value(&change.after),
        "outcome": match change.outcome {
            GrantOutcome::Changed => "changed",
            GrantOutcome::Unchanged => "unchanged",
            GrantOutcome::NotWritten => "not_written",
        },
    })
}

fn operation(operation: &GrantOperation) -> Value {
    match operation {
        GrantOperation::Grant { agents, commands } => {
            let mut value = json!({"op": "grant"});
            match agents {
                Some(Agents::All) => value["agents"] = json!("*"),
                Some(Agents::Named(names)) => {
                    value["agents"] = names.iter().map(|name| name.as_str()).collect();
                }
                None => {}
            }
            if let Some(commands) = commands {
                value["maxTimeoutMs"] = json!(commands.max_timeout_ms());
            }
            value
        }
        GrantOperation::UngrantAgent(agent) => json!({"op": "ungrant", "agent": agent.as_str()}),
        GrantOperation::UngrantCommands => json!({"op": "ungrant", "commands": true}),
        GrantOperation::UngrantAll => json!({"op": "ungrant", "all": true}),
    }
}

#[cfg(test)]
#[path = "../../../tests/env_serve/grant_audit.rs"]
mod tests;
