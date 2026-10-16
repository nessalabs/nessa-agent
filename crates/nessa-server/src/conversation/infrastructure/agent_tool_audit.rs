//! Immutable, acknowledged source/target admission records for agent tools.
use super::mode_audit::write_once;
use crate::conversation::application::{
    AgentToolAudit, AgentToolAuditRecord, ConversationError, ConversationFuture,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::create_directory;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

pub struct DurableAgentToolAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl DurableAgentToolAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}
impl AgentToolAudit for DurableAgentToolAudit {
    fn record(&self, record: AgentToolAuditRecord) -> ConversationFuture<'_, ()> {
        let key = serde_json::to_vec(&(record.source.to_string(), &record.call, &record.tool))
            .expect("strings serialize");
        let id = format!("agent-tool-{:x}", Sha256::digest(key));
        let value = json!({
            "recordId": id, "kind": "agent_tool_decision",
            "source": { "conversationId": record.source.to_string(), "executionId": record.execution, "leaseId": record.lease },
            "target": record.target.map(|id| id.to_string()),
            "call": record.call, "requestId": record.request, "commandId": record.command, "tool": record.tool,
            "initiator": { "principalId": record.principal, "surfaceId": record.surface, "ownerInitiated": record.owner_initiated },
            "decision": { "allowed": record.allowed, "policies": record.policies, "reason": record.reason },
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        Box::pin(write_once(self.directory.clone(), id, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_protocol::conversation::domain::ConversationId;

    struct ClockAt(u64);
    impl Clock for ClockAt {
        fn unix_milliseconds(&self) -> u64 {
            self.0
        }
    }
    fn record() -> AgentToolAuditRecord {
        AgentToolAuditRecord {
            source: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            target: Some(ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap()),
            execution: "source-execution".into(),
            lease: "source-lease".into(),
            call: "call".into(),
            request: "request".into(),
            command: "scoped-command".into(),
            tool: "send_message".into(),
            principal: "owner".into(),
            surface: "panel".into(),
            owner_initiated: true,
            allowed: true,
            policies: vec!["owner.agent_tools".into()],
            reason: None,
        }
    }
    #[tokio::test]
    async fn agent_tool_decisions_preserve_source_target_actor_and_are_immutable() {
        let root = tempfile::tempdir().unwrap();
        let audit =
            DurableAgentToolAudit::new(root.path().join("audit"), Arc::new(ClockAt(42))).unwrap();
        audit.record(record()).await.unwrap();
        audit.record(record()).await.unwrap();
        let files: Vec<_> = std::fs::read_dir(root.path().join("audit"))
            .unwrap()
            .map(|file| file.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
        assert_eq!(value["source"]["executionId"], "source-execution");
        assert_eq!(value["source"]["leaseId"], "source-lease");
        assert_eq!(value["target"], "00000000-0000-4000-8000-000000000002");
        assert_eq!(value["initiator"]["principalId"], "owner");
        assert_eq!(value["requestId"], "request");
        assert_eq!(value["commandId"], "scoped-command");
        assert_eq!(value["decision"]["policies"], json!(["owner.agent_tools"]));
        let mut changed = record();
        changed.allowed = false;
        assert!(matches!(
            audit.record(changed).await,
            Err(ConversationError::Audit)
        ));
        let mut changed_request = record();
        changed_request.request = "another-logical-request".into();
        assert!(matches!(
            audit.record(changed_request).await,
            Err(ConversationError::Audit)
        ));
        let mut changed_command = record();
        changed_command.command = "another-scoped-command".into();
        assert!(matches!(
            audit.record(changed_command).await,
            Err(ConversationError::Audit)
        ));
    }
    #[tokio::test]
    async fn an_unwritable_agent_tool_audit_returns_a_visible_failure() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("audit");
        let audit = DurableAgentToolAudit::new(destination.clone(), Arc::new(ClockAt(42))).unwrap();
        std::fs::remove_dir(&destination).unwrap();
        std::fs::write(destination, b"not a directory").unwrap();
        assert!(matches!(
            audit.record(record()).await,
            Err(ConversationError::Audit)
        ));
    }
}
