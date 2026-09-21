//! Durable warm-up evidence, committed before a completion record is written.
use crate::agent_warm_up::application::{
    ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture,
};
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use nessa_sdk::application::agent_execution::{agents::AgentError, permissions::ActionContext};
use serde_json::json;
use serde_json::Value;
use std::{fmt::Display, io::Write, path::PathBuf};
use uuid::Uuid;

/// Writes one immutable record per warm-up attempt.
pub struct DurableWarmUpAudit {
    directory: PathBuf,
}

impl DurableWarmUpAudit {
    /// Open, creating the directory when it does not exist.
    ///
    /// # Errors
    /// Fails when the directory cannot be created.
    pub fn new(directory: PathBuf) -> Result<Self, WarmUpError> {
        create_directory(&directory).map_err(|error| WarmUpError::Audit(error.to_string()))?;
        Ok(Self { directory })
    }
}

fn actor(actor: &ActionContext) -> Value {
    json!({
        "principalId": actor.principal_id(),
        "surfaceId": actor.surface_id(),
        "requestId": actor.request_id(),
    })
}

/// Discriminated, like every other audit record in this repository: a reader
/// branches on `kind` rather than parsing a rendering of a Rust enum. The
/// startup step is the part a failed warm-up is usually read for, so it is a
/// field rather than prose.
fn failure(failure: &ProviderFailure) -> Value {
    let error = match &failure.error {
        AgentError::StartupDeadline(step) => json!({
            "kind": "startup_deadline",
            "step": step.as_str(),
            "context": step.context().as_str(),
        }),
        AgentError::Deadline => json!({"kind": "deadline"}),
        AgentError::Closed => json!({"kind": "closed"}),
        AgentError::CleanupUncertain => json!({"kind": "cleanup_uncertain"}),
        AgentError::AuditFailure => json!({"kind": "audit_failure"}),
        AgentError::Provider { code } => json!({"kind": "provider", "code": code}),
        AgentError::Storage(error) => {
            json!({"kind": "storage", "diagnostic": error.to_string()})
        }
        AgentError::Transport(detail) => json!({"kind": "transport", "diagnostic": detail}),
        AgentError::Protocol(detail) => json!({"kind": "protocol", "diagnostic": detail}),
        AgentError::Configuration(detail) => {
            json!({"kind": "configuration", "diagnostic": detail})
        }
        AgentError::Unsupported(detail) => json!({"kind": "unsupported", "diagnostic": detail}),
        AgentError::InvalidInput(detail) => {
            json!({"kind": "invalid_input", "diagnostic": detail})
        }
        // Reachable only if the SDK grows a failure a warm-up can hit; labelled
        // as unclassified rather than silently rendered as one of the above.
        other => json!({"kind": "other", "diagnostic": other.to_string()}),
    };
    json!({"error": error, "cleanupUnconfirmed": failure.cleanup_unconfirmed})
}

impl WarmUpAudit for DurableWarmUpAudit {
    fn record(&self, record: WarmUpAuditRecord) -> WarmUpFuture<'_, ()> {
        let id = Uuid::new_v4().to_string();
        let value = json!({
            "recordId": id,
            "kind": "agent_runtime_warm_up",
            "target": {
                "provider": record.runtime.provider(),
                "model": record.runtime.model(),
                "configuration": record.runtime.configuration(),
                "sessionId": record.session_id,
            },
            "transition": {
                "before": record.before.as_str(),
                "after": record.after.as_str(),
            },
            // Both supplied by the application. Nobody asked for this, and the
            // initiator is the same context the provider session was closed
            // with, so the SDK's closure evidence cannot name someone else.
            "cause": record.cause.as_str(),
            "initiator": actor(&record.initiator),
            "failure": record.failure.as_ref().map(failure),
            "correlationId": record.correlation_id,
            "requestedAtMs": record.requested_at_ms,
            "observedAtMs": record.observed_at_ms,
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let failed = |error: &dyn Display| WarmUpError::Audit(error.to_string());
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|error| failed(&error))?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|error| failed(&error))?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|()| file.as_file().sync_all())
                    .map_err(|error| failed(&error))?;
                file.persist(&directory.join(format!("{id}.json")))
                    .map_err(|error| failed(&error))?;
                sync_directory(&directory).map_err(|error| failed(&error))
            })
            .await
            .map_err(|error| WarmUpError::Audit(error.to_string()))?
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/agent_warm_up/audit.rs"]
mod tests;
