//! Durable evidence of each decision on a gateway tool call of a Drive
//! grant's turn: one immutable file per conversation, call, tool and outcome.
use super::mode_audit::write_once;
use crate::conversation::application::{
    ConversationError, ConversationFuture, DriveToolAudit, DriveToolAuditRecord,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::create_directory;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

pub struct DurableDriveToolAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl DurableDriveToolAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}
impl DriveToolAudit for DurableDriveToolAudit {
    fn record(&self, record: DriveToolAuditRecord) -> ConversationFuture<'_, ()> {
        let outcome = if record.allowed { "allowed" } else { "refused" };
        let correlation = format!(
            "{}:{}:{}:{outcome}",
            record.conversation_id, record.call, record.tool
        );
        let digest = Sha256::digest(correlation.as_bytes());
        let id = format!("drive-tool-{digest:x}");
        let value = json!({
            "recordId": id,
            "kind": "drive_tool_decision",
            "target": {
                "conversationId": record.conversation_id.to_string(),
                "turn": record.turn,
                "call": record.call,
                "tool": record.tool,
            },
            "decision": {
                "outcome": outcome,
                "rules": record.rules,
                "reason": record.reason,
            },
            "cause": "tool_call",
            "initiator": {
                "kind": "grant",
                "grant": record.grant,
                "policyRevision": record.revision,
                "principalId": record.initiator_principal_id,
                "surfaceId": record.initiator_surface_id,
            },
            // When the adapter observed the decision for durable delivery.
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        Box::pin(write_once(self.directory.clone(), id, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_protocol::conversation::domain::ConversationId;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestClock(AtomicU64);
    impl Clock for TestClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.fetch_add(1, Ordering::SeqCst)
        }
    }

    fn record(allowed: bool) -> DriveToolAuditRecord {
        DriveToolAuditRecord {
            conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000826").unwrap(),
            turn: "turn-1".into(),
            call: "call-1".into(),
            initiator_principal_id: "driver".into(),
            initiator_surface_id: "panel".into(),
            grant: "grant-1".into(),
            revision: 3,
            tool: "run".into(),
            allowed,
            rules: vec!["grant-allows-tool".into()],
            reason: (!allowed).then(|| "refused".into()),
        }
    }

    fn audit(directory: &std::path::Path) -> DurableDriveToolAudit {
        DurableDriveToolAudit::new(directory.join("drive-tools"), Arc::new(TestClock(1.into())))
            .unwrap()
    }

    #[tokio::test]
    async fn a_decision_is_written_once_and_acknowledged_again() {
        let root = tempfile::tempdir().unwrap();
        let audit = audit(root.path());
        audit.record(record(true)).await.unwrap();
        audit.record(record(true)).await.unwrap();
        let files: Vec<_> = std::fs::read_dir(root.path().join("drive-tools"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
        assert_eq!(stored["decision"]["outcome"], "allowed");
        assert_eq!(stored["decision"]["rules"], json!(["grant-allows-tool"]));
        assert_eq!(stored["initiator"]["grant"], "grant-1");
        assert_eq!(stored["initiator"]["policyRevision"], 3);
        assert_eq!(stored["initiator"]["principalId"], "driver");
        assert_eq!(stored["target"]["tool"], "run");
        assert_eq!(stored["target"]["call"], "call-1");
    }

    #[tokio::test]
    async fn a_contradicting_record_for_the_same_decision_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let audit = audit(root.path());
        audit.record(record(false)).await.unwrap();
        let mut other = record(false);
        other.reason = Some("another reason".into());
        assert!(matches!(
            audit.record(other).await,
            Err(ConversationError::Audit)
        ));
    }
}
