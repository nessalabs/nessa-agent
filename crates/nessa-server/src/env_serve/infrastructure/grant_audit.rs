//! Every change this host's owner makes to what a gateway is granted, kept
//! for good: `<data>/environment/grants-audit.jsonl`, one JSON line per
//! record, beside the grants it audits.
//!
//! ```text
//! {"schemaVersion": 1, "committedAtMs": 1760000000000, "initiator": "unix:1000",
//!  "peer": "ssh", "operation": {"op": "grant", "agents": ["claude"]},
//!  "before": {"agents": "*", "commands": {"maxTimeoutMs": 3600000}},
//!  "after": {...}, "outcome": "intended"}
//! ```
//!
//! ```text
//! FileGrantAudit::record ──▶ open (private, O_APPEND) ──▶ the line written
//!   in one write ──▶ synced ──▶ Ok
//! ```
//!
//! Arrows are steps, in order. `before` and `after` are the peer's whole
//! grant, its defaults included. Every record is written while grants.lock
//! is held, and with `O_APPEND`, so records are in the order the changes
//! were made and no two lines overwrite each other. Lines are only ever
//! appended, never rewritten.
//!
//! **What a change leaves here.** A change that leaves the grant as it was
//! leaves one `unchanged` record. Any other leaves two:
//!
//! - `intended`, kept before grants.json is replaced. If it cannot be kept,
//!   grants.json is left as it was and the command fails.
//! - then the outcome: `written` once grants.json was replaced, with
//!   `durable: false` when the directory could not then be synced (the
//!   command fails, saying the change is in force but not yet durable);
//!   or `not_written` when it could not be replaced (the grant is still
//!   `before`, and the command fails).
//!
//! An `intended` record with nothing after it is a change whose process
//! died between the two, or whose outcome record could not be kept (the
//! command then fails, saying the change is in force but unconfirmed): it
//! may or may not be in force, and grants.json is the authority for what
//! is. The journal is not bounded: it grows by two lines, each at most
//! about two grants entries long, for each change the owner makes by hand,
//! which no gateway can cause.
use super::grants::{grant_value, peer_name};
use crate::env_serve::{
    application::{GrantAudit, GrantChange, GrantOutcome},
    domain::{Agents, GrantOperation},
};
use nessa_local_storage::verify_file;
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::OpenOptionsExt,
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
        // O_APPEND: each write lands at the end as it is made, whoever else
        // has the journal open.
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&self.path)
            .map_err(failed)?;
        verify_file(&file).map_err(failed)?;
        file.write_all(&line).map_err(failed)?;
        file.sync_data().map_err(failed)
    }
}

fn line(change: &GrantChange) -> Value {
    let mut line = json!({
        "schemaVersion": SCHEMA_VERSION,
        "committedAtMs": change.committed_at_ms,
        "initiator": change.initiator,
        "peer": peer_name(&change.peer),
        "operation": operation(&change.operation),
        "before": grant_value(&change.before),
        "after": grant_value(&change.after),
        "outcome": match change.outcome {
            GrantOutcome::Intended => "intended",
            GrantOutcome::Unchanged => "unchanged",
            GrantOutcome::Written { .. } => "written",
            GrantOutcome::NotWritten => "not_written",
        },
    });
    if let GrantOutcome::Written { durable } = change.outcome {
        line["durable"] = json!(durable);
    }
    line
}

fn operation(operation: &GrantOperation) -> Value {
    match operation {
        GrantOperation::Grant {
            agents,
            commands,
            max_lease,
        } => {
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
            if let Some(max_lease) = max_lease {
                value["maxLeaseMs"] = json!(max_lease.ms());
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
