//! Immutable evidence that a remembered permission rule was revoked.
//!
//! One file per rule id and revision, written before the row is deleted.
//! The same revocation recorded again is acknowledged. A record that names
//! a different actor or a different rule is refused, and the row stays.

use nessa_auth::application::ports::Clock;
use nessa_local_storage::create_directory;
use nessa_sdk::domain::agent_execution::permissions::PermissionEffect;
use serde_json::json;
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
    fn record(&self, revocation: &PermissionRuleRevocation) -> Result<(), RuleCatalogError> {
        let correlation = format!("{}:{}", revocation.id, revocation.revision);
        let digest = Sha256::digest(correlation.as_bytes());
        let id = format!("permission-rule-revoke-{digest:x}");
        let effect = match revocation.effect {
            PermissionEffect::Allow => "allow",
            PermissionEffect::Deny => "deny",
        };
        let value = json!({
            "recordId": id,
            "kind": "permission_rule_revocation",
            "target": {
                "ruleId": revocation.id,
                "revision": revocation.revision,
                "workspace": revocation.workspace,
                "source": revocation.source,
                "tool": revocation.tool,
            },
            "transition": {
                "before": effect,
                "after": "absent",
            },
            "cause": "caller_requested",
            "initiator": {
                "kind": "person",
                "principalId": revocation.actor.principal_id(),
                "surfaceId": revocation.actor.surface_id(),
            },
            "correlationId": revocation.actor.request_id(),
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        commit_once(&self.directory, &id, &value).map_err(|_| RuleCatalogError::Unavailable)
    }
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
}
