//! Durable evidence of each window change an agent or a person's CLI asked
//! for, in `<namespace>/conversations/audit/window-control`: one private JSON
//! file per record, named by its operation and step, written once and
//! synced, as the settings audit beside it is.
use super::mode_audit::write_once;
use crate::application::{
    ConversationError, ConversationFuture, WindowControlAudit, WindowControlAuditRecord,
    WindowControlOutcome, WindowControlVia,
};
use crate::domain::WindowAction;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::create_directory;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

pub struct DurableWindowControlAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableWindowControlAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}

/// The action as kept: its name and what it names.
pub fn action(action: &WindowAction) -> Value {
    let mut value = json!({ "kind": action.name() });
    match action {
        WindowAction::Show { open, .. } => value["open"] = json!(open),
        WindowAction::PanelTab(widget) => {
            value["widget"] = json!({ "plugin": widget.plugin(), "id": widget.id() });
        }
        WindowAction::View(view) => value["view"] = json!(view.name()),
        WindowAction::Open {
            conversation,
            place,
        } => {
            value["conversationId"] = json!(conversation.to_string());
            value["place"] = json!(place.name());
        }
        WindowAction::Focus(conversation) | WindowAction::Close(conversation) => {
            value["conversationId"] = json!(conversation.to_string());
        }
    }
    value
}

/// The record as kept, without its id and observation time.
pub fn stored(record: &WindowControlAuditRecord) -> Value {
    let (step, outcome) = match &record.outcome {
        WindowControlOutcome::Requested => ("requested", Value::Null),
        WindowControlOutcome::Applied { revision } => (
            "outcome",
            json!({ "outcome": "applied", "revision": revision }),
        ),
        WindowControlOutcome::Unchanged { revision } => (
            "outcome",
            json!({ "outcome": "unchanged", "revision": revision }),
        ),
        WindowControlOutcome::Refused { code } => {
            ("outcome", json!({ "outcome": "refused", "code": code }))
        }
    };
    let via = match &record.via {
        WindowControlVia::Agent {
            source,
            execution,
            call,
        } => json!({
            "kind": "agent_tool",
            "conversationId": source.to_string(),
            "executionId": execution,
            "call": call,
        }),
        WindowControlVia::Session => json!({ "kind": "session" }),
    };
    json!({
        "kind": "window_control",
        "operationId": record.operation_id,
        "phase": step,
        "windowId": record.window_id,
        "action": action(&record.action),
        "severity": match record.action.severity() {
            crate::domain::WindowActionSeverity::Arrange => "arrange",
            crate::domain::WindowActionSeverity::Destructive => "destructive",
        },
        "policies": record.policies,
        "outcome": outcome,
        "via": via,
        "initiator": {
            "organizationId": record.caller.organization_id.as_str(),
            "principalId": record.caller.principal_id.as_str(),
            "surfaceId": record.caller.surface_id,
        },
        "correlationId": record.caller.action_id,
    })
}

impl WindowControlAudit for DurableWindowControlAudit {
    fn record(&self, record: WindowControlAuditRecord) -> ConversationFuture<'_, ()> {
        let step = match record.outcome {
            WindowControlOutcome::Requested => "requested",
            _ => "outcome",
        };
        let id = format!("window-control-{}-{step}", record.operation_id);
        let mut value = stored(&record);
        value["recordId"] = json!(id);
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        Box::pin(write_once(self.directory.clone(), id, value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ConversationCaller;
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use nessa_protocol::conversation::domain::ConversationId;

    struct ClockAt(u64);
    impl Clock for ClockAt {
        fn unix_milliseconds(&self) -> u64 {
            self.0
        }
    }

    fn record(outcome: WindowControlOutcome) -> WindowControlAuditRecord {
        WindowControlAuditRecord {
            operation_id: "op-1".into(),
            caller: ConversationCaller {
                organization_id: OrganizationId::new("org").unwrap(),
                principal_id: PrincipalId::new("owner").unwrap(),
                surface_id: "agent-tool".into(),
                action_id: "call-1".into(),
            },
            via: WindowControlVia::Agent {
                source: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
                execution: "execution-1".into(),
                call: "call-1".into(),
            },
            window_id: Some("00000000-0000-4000-8000-0000000000aa".into()),
            action: WindowAction::Close(
                ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap(),
            ),
            policies: vec!["owner.workspace_control".into()],
            outcome,
        }
    }

    #[tokio::test]
    async fn each_change_keeps_its_request_and_its_outcome_once() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurableWindowControlAudit::new(root.path().join("audit"), Arc::new(ClockAt(7)))
            .unwrap();
        audit
            .record(record(WindowControlOutcome::Requested))
            .await
            .unwrap();
        audit
            .record(record(WindowControlOutcome::Applied { revision: 4 }))
            .await
            .unwrap();
        // The same step again is acknowledged, not written twice.
        audit
            .record(record(WindowControlOutcome::Requested))
            .await
            .unwrap();
        let mut files: Vec<_> = std::fs::read_dir(root.path().join("audit"))
            .unwrap()
            .map(|file| file.unwrap().path())
            .collect();
        files.sort();
        assert_eq!(files.len(), 2);
        let outcome: Value =
            serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        assert_eq!(outcome["phase"], "outcome");
        assert_eq!(outcome["severity"], "destructive");
        assert_eq!(outcome["action"]["kind"], "close_pane");
        assert_eq!(outcome["outcome"]["revision"], 4);
        assert_eq!(outcome["via"]["kind"], "agent_tool");
    }
}
