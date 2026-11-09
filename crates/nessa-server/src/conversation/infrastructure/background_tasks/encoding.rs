//! Persisted representation is private to the task store; domain restoration
//! validates lifecycle agreement before these bytes can cause any effect.
use crate::conversation::{
    application::{
        background_tasks::{BackgroundRecord, BackgroundSource, BackgroundTaskError},
        ConversationCaller,
    },
    domain::{
        BackgroundCommand, BackgroundTaskId, TaskCause, TaskDelivery, TaskInitiator, TaskLifecycle,
        TaskOutcome, TaskStop,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    task: String,
    conversation: String,
    execution: String,
    tool_call: String,
    organization: String,
    principal: String,
    surface: String,
    action: String,
    workspace: String,
    command: String,
    timeout_seconds: u64,
    admitted_at_ms: u64,
    ended_at_ms: Option<u64>,
    outcome: Option<Outcome>,
    stop: Option<Stop>,
    delivery: Delivery,
    revision: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Outcome {
    cause: Cause,
    scope: Option<u64>,
    process: Option<u64>,
    os_pid: Option<u32>,
    exit_code: Option<i32>,
    signal: Option<String>,
    cleanup_verified: bool,
    stdout: String,
    stderr: String,
    omitted_bytes: u64,
    output_complete: bool,
    error: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cause {
    Exited,
    Cancelled,
    TimedOut,
    SpawnFailed,
    WorkerFailed,
    Interrupted,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Delivery {
    Waiting,
    Delivered,
    Discarded,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
enum Stop {
    Requested {
        execution: String,
        initiator: Initiator,
    },
    ConversationDeleted {
        initiator: Initiator,
    },
    GatewayStopped {
        initiator: Initiator,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Initiator {
    principal: String,
    surface: String,
    action: String,
}
impl Initiator {
    fn encode(value: &TaskInitiator) -> Self {
        Self {
            principal: value.principal().into(),
            surface: value.surface().into(),
            action: value.action().into(),
        }
    }
    fn decode(self) -> Result<TaskInitiator, BackgroundTaskError> {
        TaskInitiator::new(self.principal, self.surface, self.action)
            .map_err(|_| BackgroundTaskError::Storage)
    }
}
impl Stop {
    fn encode(value: &TaskStop) -> Self {
        match value {
            TaskStop::Requested {
                execution,
                initiator,
            } => Self::Requested {
                execution: execution.to_string(),
                initiator: Initiator::encode(initiator),
            },
            TaskStop::ConversationDeleted(initiator) => Self::ConversationDeleted {
                initiator: Initiator::encode(initiator),
            },
            TaskStop::GatewayStopped(initiator) => Self::GatewayStopped {
                initiator: Initiator::encode(initiator),
            },
        }
    }
    fn decode(self) -> Result<TaskStop, BackgroundTaskError> {
        Ok(match self {
            Self::Requested {
                execution,
                initiator,
            } => TaskStop::Requested {
                execution: execution.into(),
                initiator: initiator.decode()?,
            },
            Self::ConversationDeleted { initiator } => {
                TaskStop::ConversationDeleted(initiator.decode()?)
            }
            Self::GatewayStopped { initiator } => TaskStop::GatewayStopped(initiator.decode()?),
        })
    }
}
impl Cause {
    fn encode(value: TaskCause) -> Self {
        match value {
            TaskCause::Exited => Self::Exited,
            TaskCause::Cancelled => Self::Cancelled,
            TaskCause::TimedOut => Self::TimedOut,
            TaskCause::SpawnFailed => Self::SpawnFailed,
            TaskCause::WorkerFailed => Self::WorkerFailed,
            TaskCause::Interrupted => Self::Interrupted,
        }
    }
    fn decode(self) -> TaskCause {
        match self {
            Self::Exited => TaskCause::Exited,
            Self::Cancelled => TaskCause::Cancelled,
            Self::TimedOut => TaskCause::TimedOut,
            Self::SpawnFailed => TaskCause::SpawnFailed,
            Self::WorkerFailed => TaskCause::WorkerFailed,
            Self::Interrupted => TaskCause::Interrupted,
        }
    }
}
impl Delivery {
    fn encode(value: TaskDelivery) -> Self {
        match value {
            TaskDelivery::Waiting => Self::Waiting,
            TaskDelivery::Delivered => Self::Delivered,
            TaskDelivery::Discarded => Self::Discarded,
        }
    }
    fn decode(self) -> TaskDelivery {
        match self {
            Self::Waiting => TaskDelivery::Waiting,
            Self::Delivered => TaskDelivery::Delivered,
            Self::Discarded => TaskDelivery::Discarded,
        }
    }
}
impl Outcome {
    fn encode(value: &TaskOutcome) -> Self {
        Self {
            cause: Cause::encode(value.cause),
            scope: value.scope,
            process: value.process,
            os_pid: value.os_pid,
            exit_code: value.exit_code,
            signal: value.signal.clone(),
            cleanup_verified: value.cleanup_verified,
            stdout: value.stdout.clone(),
            stderr: value.stderr.clone(),
            omitted_bytes: value.omitted_bytes,
            output_complete: value.output_complete,
            error: value.error.clone(),
        }
    }
    fn decode(self) -> Result<TaskOutcome, BackgroundTaskError> {
        if self.stdout.len() + self.stderr.len() > TaskOutcome::MAX_OUTPUT_BYTES * 3 {
            return Err(BackgroundTaskError::Storage);
        }
        Ok(TaskOutcome {
            cause: self.cause.decode(),
            scope: self.scope,
            process: self.process,
            os_pid: self.os_pid,
            exit_code: self.exit_code,
            signal: self.signal,
            cleanup_verified: self.cleanup_verified,
            stdout: self.stdout,
            stderr: self.stderr,
            omitted_bytes: self.omitted_bytes,
            output_complete: self.output_complete,
            error: self.error,
        })
    }
}
pub(super) fn encode(value: &BackgroundRecord) -> Result<String, BackgroundTaskError> {
    let source = &value.source;
    let state = &value.lifecycle;
    serde_json::to_string(&Record {
        task: value.id.as_str().into(),
        conversation: source.conversation.to_string(),
        execution: source.execution.clone(),
        tool_call: source.tool_call.clone(),
        organization: source.caller.organization_id.as_str().into(),
        principal: source.caller.principal_id.as_str().into(),
        surface: source.caller.surface_id.clone(),
        action: source.caller.action_id.clone(),
        workspace: source.workspace.clone(),
        command: value.command.text().into(),
        timeout_seconds: value.command.timeout_seconds(),
        admitted_at_ms: state.admitted_at_ms(),
        ended_at_ms: state.ended_at_ms(),
        outcome: state.outcome().map(Outcome::encode),
        stop: state.stop().map(Stop::encode),
        delivery: Delivery::encode(state.delivery()),
        revision: state.revision(),
    })
    .map_err(|_| BackgroundTaskError::Storage)
}
pub(super) fn decode(value: &str) -> Result<BackgroundRecord, BackgroundTaskError> {
    if value.len() > 256 * 1024 {
        return Err(BackgroundTaskError::Storage);
    }
    let record: Record = serde_json::from_str(value).map_err(|_| BackgroundTaskError::Storage)?;
    let state = TaskLifecycle::restore(
        record.admitted_at_ms,
        record.ended_at_ms,
        record.outcome.map(Outcome::decode).transpose()?,
        record.stop.map(Stop::decode).transpose()?,
        record.delivery.decode(),
        record.revision,
    )
    .map_err(|_| BackgroundTaskError::Storage)?;
    Ok(BackgroundRecord {
        id: BackgroundTaskId::new(record.task).map_err(|_| BackgroundTaskError::Storage)?,
        command: BackgroundCommand::new(record.command, record.timeout_seconds)
            .map_err(|_| BackgroundTaskError::Storage)?,
        source: BackgroundSource {
            conversation: ConversationId::new(&record.conversation)
                .map_err(|_| BackgroundTaskError::Storage)?,
            execution: record.execution,
            tool_call: record.tool_call,
            caller: ConversationCaller {
                organization_id: OrganizationId::new(&record.organization)
                    .map_err(|_| BackgroundTaskError::Storage)?,
                principal_id: PrincipalId::new(&record.principal)
                    .map_err(|_| BackgroundTaskError::Storage)?,
                surface_id: record.surface,
                action_id: record.action,
            },
            workspace: record.workspace,
        },
        lifecycle: state,
    })
}
pub(super) fn evidence(
    before: Option<&BackgroundRecord>,
    after: &BackgroundRecord,
) -> Result<String, BackgroundTaskError> {
    Ok(serde_json::json!({
        "admission":before.is_none().then(|| serde_json::json!({
            "command":after.command.text(), "timeoutSeconds":after.command.timeout_seconds(),
            "workspace":after.source.workspace, "execution":after.source.execution,
            "toolCall":after.source.tool_call, "organization":after.source.caller.organization_id.as_str(),
            "principal":after.source.caller.principal_id.as_str(), "surface":after.source.caller.surface_id,
            "action":after.source.caller.action_id,
        })),
        "beforeRevision":before.map(|r| r.lifecycle.revision()), "afterRevision":after.lifecycle.revision(),
        "sourcePrincipal":after.source.caller.principal_id.as_str(), "sourceExecution":after.source.execution, "sourceAction":after.source.caller.action_id,
        "stop":after.lifecycle.stop().map(Stop::encode), "outcome":after.lifecycle.outcome().map(|o| Cause::encode(o.cause)),
        "cleanupVerified":after.lifecycle.outcome().map(|o| o.cleanup_verified),
        "process":after.lifecycle.outcome().map(|o| serde_json::json!({"scope":o.scope,"process":o.process,"osPid":o.os_pid,"exitCode":o.exit_code,"signal":o.signal,"outputComplete":o.output_complete,"omittedBytes":o.omitted_bytes})),
        "beforeDelivery":before.map(|r| Delivery::encode(r.lifecycle.delivery())), "afterDelivery":Delivery::encode(after.lifecycle.delivery()),
        "admittedAtMs":after.lifecycle.admitted_at_ms(), "endedAtMs":after.lifecycle.ended_at_ms(),
    }).to_string())
}
pub(super) fn erasure_evidence(record: &BackgroundRecord, actor: &TaskInitiator) -> String {
    serde_json::json!({
        "operation":"conversation_deleted", "beforeRevision":record.lifecycle.revision(), "after":"erased",
        "initiator":Initiator::encode(actor), "outcome":record.lifecycle.outcome().map(|o| Cause::encode(o.cause)),
        "cleanupVerified":record.lifecycle.outcome().is_some_and(|o| o.cleanup_verified),
    }).to_string()
}
