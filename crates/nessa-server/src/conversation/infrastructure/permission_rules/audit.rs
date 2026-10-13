//! Immutable evidence that a remembered permission rule was revoked.
//!
//! A request file is written while the row is still there. It does not say
//! the rule is gone. The absent file is written after the delete. The same
//! principal recording the same revision again is acknowledged, including
//! when the request id differs. A different principal is refused.

use nessa_auth::application::ports::Clock;
use nessa_local_storage::create_directory;
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

use crate::conversation::application::{
    PermissionRuleRevocation, PermissionRuleRevocationAudit, RuleCatalogError,
};
use crate::conversation::infrastructure::mode_audit::commit_once;

/// Files under `directory`, one per revoked rule revision.
pub struct DurablePermissionRuleRevocationAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurablePermissionRuleRevocationAudit {
    /// Create `directory` and keep revocations there. The clock dates the
    /// observation, not the unknown moment the person decided.
    pub fn new(
        directory: PathBuf,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, crate::conversation::application::ConversationError> {
        create_directory(&directory)
            .map_err(|_| crate::conversation::application::ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}

impl PermissionRuleRevocationAudit for DurablePermissionRuleRevocationAudit {
    fn requested(&self, revocation: &PermissionRuleRevocation) -> Result<(), RuleCatalogError> {
        let id = requested_name(&revocation.id, &revocation.revision);
        let value = self.evidence(id.as_str(), revocation, false);
        commit_revocation(&self.directory, &id, &value)
    }

    fn record(&self, revocation: &PermissionRuleRevocation) -> Result<(), RuleCatalogError> {
        let id = removed_name(&revocation.id, &revocation.revision);
        let value = self.evidence(id.as_str(), revocation, true);
        commit_revocation(&self.directory, &id, &value)
    }

    fn finish(&self, id: &str, actor: &ActionContext) -> Result<bool, RuleCatalogError> {
        let Some(value) = find_requested(&self.directory, id)? else {
            return Ok(false);
        };
        let principal = value["initiator"]["principalId"].as_str().unwrap_or("");
        if principal != actor.principal_id() {
            return Err(RuleCatalogError::Unavailable);
        }
        let revocation = revocation_from(&value)?;
        let removed = removed_name(&revocation.id, &revocation.revision);
        let destination = self.directory.join(format!("{removed}.json"));
        let evidence = self.evidence(&removed, &revocation, true);
        if revocation_agrees(&destination, &evidence)? {
            return Ok(false);
        }
        self.record(&revocation)?;
        Ok(true)
    }
}

impl DurablePermissionRuleRevocationAudit {
    fn evidence(&self, id: &str, revocation: &PermissionRuleRevocation, removed: bool) -> Value {
        let effect = match revocation.effect {
            PermissionEffect::Allow => "allow",
            PermissionEffect::Deny => "deny",
        };
        json!({
            "recordId": id,
            "kind": if removed {
                "permission_rule_revocation"
            } else {
                "permission_rule_revocation_requested"
            },
            "target": {
                "ruleId": revocation.id,
                "revision": revocation.revision,
                "workspace": revocation.workspace,
                "source": revocation.source,
                "tool": revocation.tool,
            },
            "transition": {
                "before": effect,
                "after": if removed { "absent" } else { effect },
            },
            "cause": "caller_requested",
            "initiator": {
                "kind": "person",
                "principalId": revocation.actor.principal_id(),
                "surfaceId": revocation.actor.surface_id(),
            },
            "correlationId": revocation.actor.request_id(),
            "observedAtMs": self.clock.unix_milliseconds(),
        })
    }
}

fn requested_name(id: &str, revision: &str) -> String {
    let digest = Sha256::digest(format!("{id}:{revision}:requested").as_bytes());
    format!("permission-rule-revoke-requested-{digest:x}")
}

fn removed_name(id: &str, revision: &str) -> String {
    let digest = Sha256::digest(format!("{id}:{revision}").as_bytes());
    format!("permission-rule-revoke-{digest:x}")
}

/// Commit `value`, or acknowledge the file already there when it is the same
/// revocation apart from its time and request id. A different principal or a
/// different rule is refused.
fn commit_revocation(
    directory: &std::path::Path,
    id: &str,
    value: &serde_json::Value,
) -> Result<(), RuleCatalogError> {
    let destination = directory.join(format!("{id}.json"));
    if revocation_agrees(&destination, value)? {
        return nessa_local_storage::sync_directory(directory)
            .map_err(|_| RuleCatalogError::Unavailable);
    }
    commit_once(directory, id, value).map_err(|_| RuleCatalogError::Unavailable)
}

fn revocation_agrees(
    destination: &std::path::Path,
    expected: &serde_json::Value,
) -> Result<bool, RuleCatalogError> {
    use nessa_local_storage::OpenMode;
    use std::io::{ErrorKind, Read};
    let mut file = match nessa_local_storage::open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(RuleCatalogError::Unavailable),
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| RuleCatalogError::Unavailable)?;
    if bytes.len() > 16_384 {
        return Err(RuleCatalogError::Unavailable);
    }
    let mut stored: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| RuleCatalogError::Unavailable)?;
    let mut expected = expected.clone();
    for value in [&mut stored, &mut expected] {
        if let Some(object) = value.as_object_mut() {
            object.remove("observedAtMs");
            object.remove("correlationId");
        }
    }
    if stored != expected {
        return Err(RuleCatalogError::Unavailable);
    }
    Ok(true)
}

fn find_requested(
    directory: &std::path::Path,
    id: &str,
) -> Result<Option<serde_json::Value>, RuleCatalogError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Err(RuleCatalogError::Unavailable),
    };
    for entry in entries {
        let path = entry.map_err(|_| RuleCatalogError::Unavailable)?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("permission-rule-revoke-requested-") || !name.ends_with(".json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|_| RuleCatalogError::Unavailable)?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| RuleCatalogError::Unavailable)?;
        if value["target"]["ruleId"] == id {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn revocation_from(
    value: &serde_json::Value,
) -> Result<PermissionRuleRevocation, RuleCatalogError> {
    use nessa_sdk::application::agent_execution::permissions::ActionContext;
    let text = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .ok_or(RuleCatalogError::Unavailable)
            .map(str::to_owned)
    };
    let effect = match text("/transition/before")?.as_str() {
        "allow" => PermissionEffect::Allow,
        "deny" => PermissionEffect::Deny,
        _ => return Err(RuleCatalogError::Unavailable),
    };
    let actor = ActionContext::new(
        text("/initiator/principalId")?,
        text("/initiator/surfaceId")?,
        value["correlationId"].as_str().unwrap_or(""),
    )
    .map_err(|_| RuleCatalogError::Unavailable)?;
    Ok(PermissionRuleRevocation {
        id: text("/target/ruleId")?,
        revision: text("/target/revision")?,
        workspace: text("/target/workspace")?,
        source: text("/target/source")?,
        tool: text("/target/tool")?,
        effect,
        actor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::application::agent_execution::permissions::ActionContext;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestClock(AtomicU64);
    impl Clock for TestClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.fetch_add(1, Ordering::SeqCst)
        }
    }

    fn revocation(principal: &str) -> PermissionRuleRevocation {
        PermissionRuleRevocation {
            id: "pr_rule".into(),
            revision: "2".into(),
            workspace: "/work".into(),
            source: "claude".into(),
            tool: "Read".into(),
            effect: PermissionEffect::Deny,
            actor: ActionContext::new(principal, "surface", "request").unwrap(),
        }
    }

    #[test]
    fn a_revocation_is_written_once_and_a_contradiction_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurablePermissionRuleRevocationAudit::new(
            root.path().join("revocations"),
            Arc::new(TestClock(AtomicU64::new(10))),
        )
        .unwrap();
        let first = revocation("owner");
        audit.record(&first).unwrap();
        audit.record(&first).unwrap();
        assert_eq!(
            audit.record(&revocation("someone-else")).unwrap_err(),
            RuleCatalogError::Unavailable
        );
        let mut files: Vec<_> = std::fs::read_dir(root.path().join("revocations"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(files.pop().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["kind"], "permission_rule_revocation");
        assert_eq!(value["target"]["ruleId"], "pr_rule");
        assert_eq!(value["target"]["revision"], "2");
        assert_eq!(value["target"]["tool"], "Read");
        assert_eq!(value["transition"]["before"], "deny");
        assert_eq!(value["transition"]["after"], "absent");
        assert_eq!(value["cause"], "caller_requested");
        assert_eq!(value["initiator"]["principalId"], "owner");
        assert_eq!(value["correlationId"], "request");
    }

    #[test]
    fn a_retry_with_a_new_request_agrees_and_a_different_principal_does_not() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurablePermissionRuleRevocationAudit::new(
            root.path().join("revocations"),
            Arc::new(TestClock(AtomicU64::new(10))),
        )
        .unwrap();
        let first = revocation("owner");
        audit.requested(&first).unwrap();
        let mut retry = revocation("owner");
        retry.actor = ActionContext::new("owner", "surface", "retry").unwrap();
        audit.requested(&retry).unwrap();
        assert_eq!(
            audit.requested(&revocation("someone-else")).unwrap_err(),
            RuleCatalogError::Unavailable
        );
        let requested: Vec<_> = std::fs::read_dir(root.path().join("revocations"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("permission-rule-revoke-requested-"))
            })
            .collect();
        assert_eq!(requested.len(), 1);
        let text = std::fs::read_to_string(&requested[0]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["kind"], "permission_rule_revocation_requested");
        assert_eq!(value["transition"]["before"], "deny");
        assert_eq!(value["transition"]["after"], "deny");
        assert_eq!(value["correlationId"], "request");
        assert!(audit.finish("pr_rule", &retry.actor).unwrap());
        assert!(!audit.finish("pr_rule", &retry.actor).unwrap());
        assert!(!audit.finish("missing", &retry.actor).unwrap());
        let removed = std::fs::read_dir(root.path().join("revocations"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with("permission-rule-revoke-") && !name.contains("requested")
                })
            })
            .count();
        assert_eq!(removed, 1);
    }
}
