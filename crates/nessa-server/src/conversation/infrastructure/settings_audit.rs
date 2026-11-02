//! Durable evidence of each change made from Settings, in
//! `<namespace>/conversations/audit/settings`: one private JSON file per
//! record, named by its operation and step, written once and synced, as the
//! approval-mode audit beside it is. A change's records share its operation
//! id; between changes the only order is `observedAtMs`, on the wall clock.
use super::mode_audit::write_once;
use crate::conversation::application::{
    ConversationError, ConversationFuture, SettingsAudit, SettingsAuditRecord, SettingsChange,
    SettingsOutcome,
};
use crate::conversation::domain::ChannelExecution;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::create_directory;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

pub struct DurableSettingsAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableSettingsAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}

fn placement(execution: ChannelExecution) -> Value {
    json!({
        "coordinator": execution.coordinator.as_str(),
        "threads": execution.threads.as_str(),
    })
}

/// The record as kept, without its id and observation time.
pub(crate) fn stored(record: &SettingsAuditRecord) -> Value {
    let (kind, target, transition) = match &record.change {
        SettingsChange::AppUntrust { server } => (
            "app_untrust",
            json!({ "server": server }),
            json!({ "before": "always_allowed", "requested": "ask" }),
        ),
        SettingsChange::ChannelPlacement {
            channel,
            before,
            after,
            coordinator,
        } => (
            "channel_placement",
            json!({
                "channelId": channel.as_str(),
                "coordinatorConversationId": coordinator.to_string(),
            }),
            json!({ "before": placement(*before), "requested": placement(*after) }),
        ),
    };
    let (step, outcome) = match &record.outcome {
        SettingsOutcome::Requested => ("requested", Value::Null),
        SettingsOutcome::Applied {
            changed,
            coordinator,
        } => (
            "outcome",
            json!({
                "outcome": "applied",
                "changed": changed,
                "coordinatorConversationId": coordinator.as_ref().map(ToString::to_string),
            }),
        ),
        SettingsOutcome::Failed => ("outcome", json!({ "outcome": "failed" })),
    };
    json!({
        "kind": kind,
        "operationId": record.operation_id,
        "phase": step,
        "target": target,
        "transition": transition,
        "outcome": outcome,
        "cause": "caller_requested",
        "initiator": {
            "kind": "caller",
            "organizationId": record.caller.organization_id.as_str(),
            "principalId": record.caller.principal_id.as_str(),
            "surfaceId": record.caller.surface_id,
        },
        "correlationId": record.caller.action_id,
    })
}

impl SettingsAudit for DurableSettingsAudit {
    fn record(&self, record: SettingsAuditRecord) -> ConversationFuture<'_, ()> {
        let mut value = stored(&record);
        let step = match record.outcome {
            SettingsOutcome::Requested => "requested",
            SettingsOutcome::Applied { .. } | SettingsOutcome::Failed => "outcome",
        };
        let id = format!("settings-{}-{step}", record.operation_id);
        if let Value::Object(fields) = &mut value {
            fields.insert("recordId".into(), json!(id));
            // When this adapter observed the step, not when the change took effect.
            fields.insert("observedAtMs".into(), json!(self.clock.unix_milliseconds()));
        }
        Box::pin(write_once(self.directory.clone(), id, value))
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/settings_audit.rs"]
mod tests;
