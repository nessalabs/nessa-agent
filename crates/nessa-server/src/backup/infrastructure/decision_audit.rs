//! The inventory policy decisions, stored before the log is read.
//!
//! One JSON line per decision, in `backup/inventory-decisions.jsonl`. The
//! line names who asked, why, which action, the allow or deny, and the Cedar
//! policy ids. It does not name a person: a gateway capture has no human
//! principal. `observedAtMs` is this adapter's clock as it builds the line,
//! not the time the fence committed.
//!
//! The whole file is replaced from a private temporary file, and the file and
//! its directory are synced, before the record is acknowledged. A directory
//! sync that fails after the rename restores the previous bytes. A line that
//! is not the next `inventory-decision-N`, or a file that does not end on a
//! complete line, is refused and left as it is.
//!
//! Publication shares the inventory's directory lock so temporary cleanup
//! cannot remove the other publisher's replacement or rollback copy
//! (`a_decision_write_waits_for_inventory_rollback`).
//!
//! The decision and the deletion log are two files. A crash after this sync
//! and before the log sync leaves an allow whose fences are not in the log
//! yet. The next capture stores another decision and reconciles.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nessa_auth::application::ports::Clock;
use serde_json::json;

use super::log::{exclusive, persist};
use crate::backup::application::{
    DecisionAuditError, InventoryDecision, InventoryDecisionAudit, InventoryDenial, InventoryFacts,
};

/// `directory/inventory-decisions.jsonl`, observed by `clock`.
pub(crate) struct FileInventoryDecisions {
    directory: PathBuf,
    path: PathBuf,
    clock: Arc<dyn Clock>,
    gate: Mutex<()>,
}

impl FileInventoryDecisions {
    /// Keep decisions in `directory`. The directory already exists.
    pub(crate) fn open(directory: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> io::Result<Self> {
        let directory = directory.into();
        let path = directory.join("inventory-decisions.jsonl");
        let _ = next_sequence(&path)?;
        Ok(Self {
            directory,
            path,
            clock,
            gate: Mutex::new(()),
        })
    }
}

impl InventoryDecisionAudit for FileInventoryDecisions {
    fn record(
        &self,
        facts: &InventoryFacts<'_>,
        decision: &InventoryDecision,
    ) -> Result<(), DecisionAuditError> {
        let _gate = self
            .gate
            .lock()
            .map_err(|_| DecisionAuditError::NotStored)?;
        let _file = exclusive(&self.directory).map_err(|_| DecisionAuditError::NotStored)?;
        if self.path.exists() && !self.path.is_file() {
            return Err(DecisionAuditError::NotStored);
        }
        let next = next_sequence(&self.path).map_err(|_| DecisionAuditError::NotStored)?;
        let line = line(facts, decision, next, self.clock.unix_milliseconds())?;
        let mut text = if self.path.is_file() {
            fs::read_to_string(&self.path).map_err(|_| DecisionAuditError::NotStored)?
        } else {
            String::new()
        };
        text.push_str(&line);
        text.push('\n');
        persist(&self.directory, &self.path, &text).map_err(|_| DecisionAuditError::NotStored)?;
        Ok(())
    }
}

fn line(
    facts: &InventoryFacts<'_>,
    decision: &InventoryDecision,
    sequence: u64,
    observed_at_ms: u64,
) -> Result<String, DecisionAuditError> {
    let (outcome, policies, denial) = match decision {
        InventoryDecision::Allow { policies } => ("allow", policies.as_slice(), None),
        InventoryDecision::Deny(reason) => ("deny", policy_ids(reason), Some(denial_name(reason))),
    };
    let mut value = json!({
        "recordId": format!("inventory-decision-{sequence}"),
        "kind": "inventory_decision",
        "principalKind": facts.kind,
        "cause": facts.cause.as_str(),
        "action": facts.action.as_str(),
        "outcome": outcome,
        "policies": policies,
        "observedAtMs": observed_at_ms,
    });
    if let Some(denial) = denial {
        value["denial"] = json!(denial);
    }
    serde_json::to_string(&value).map_err(|_| DecisionAuditError::NotStored)
}

fn policy_ids(denial: &InventoryDenial) -> &[String] {
    match denial {
        InventoryDenial::Forbidden { policies } => policies,
        InventoryDenial::NotPermitted | InventoryDenial::Unavailable => &[],
    }
}

fn denial_name(denial: &InventoryDenial) -> &'static str {
    match denial {
        InventoryDenial::Forbidden { .. } => "forbidden",
        InventoryDenial::NotPermitted => "not-permitted",
        InventoryDenial::Unavailable => "unavailable",
    }
}

/// The next `inventory-decision-N`. A missing file is 1. A line that is not
/// that id, or a file that stops mid-line, is refused.
fn next_sequence(path: &Path) -> io::Result<u64> {
    if !path.is_file() {
        return Ok(1);
    }
    let text = fs::read_to_string(path)?;
    if text.is_empty() {
        return Ok(1);
    }
    if !text.ends_with('\n') {
        return Err(invalid());
    }
    let mut next = 1u64;
    for line in text.split_terminator('\n') {
        if line.is_empty() {
            return Err(invalid());
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|_| invalid())?;
        let expected = format!("inventory-decision-{next}");
        let id = value.get("recordId").and_then(|item| item.as_str());
        let kind = value.get("kind").and_then(|item| item.as_str());
        if id != Some(expected.as_str()) || kind != Some("inventory_decision") {
            return Err(invalid());
        }
        next = next.checked_add(1).ok_or_else(invalid)?;
    }
    Ok(next)
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "inventory decision")
}
