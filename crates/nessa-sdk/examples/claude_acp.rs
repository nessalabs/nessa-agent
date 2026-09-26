//! Explicit host composition for a local smoke run. Permissions default to deny.
use nessa_sdk::application::agent_execution::agents::{AgentFuture, AttachmentRequest};
use nessa_sdk::application::agent_execution::executions::{
    ExecutionAudit, ExecutionAuditRecord, ExecutionRequest, ExecutionUpdate,
};

use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalAttribution, ApprovalBasis, ApprovalModeSnapshot, PermissionAnswer,
};
use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;
use nessa_sdk::application::agent_execution::sessions::SessionManager;
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, MessageKind};
use nessa_sdk::domain::agent_execution::permissions::{
    PermissionCancellationReasonView, PermissionDecision, PermissionEffect, PermissionOfferPolicy,
    PermissionScope, PermissionStateView,
};
use nessa_sdk::domain::agent_execution::prompts::{
    PromptSource, PromptSourceKind, PromptText, SystemPromptBuilder, UserMessage,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::common::value_objects::TokenLimits;
use nessa_sdk::domain::model_metadata::entities::ModelMetadata;
use nessa_sdk::infrastructure::claude_acp::sessions::ClaudeAcpProvider;
use nessa_sdk::infrastructure::model_metadata_json::load_catalog;
use nessa_sdk::infrastructure::session_storage::LocalFileStorage;
use nessa_sdk::infrastructure::{acp::sessions::AcpConfig, clock::RuntimeClock};
use nessa_sdk::Agent;
use std::{
    collections::BTreeMap, error::Error, fs::File, path::PathBuf, sync::Arc, time::Duration,
};

// This smoke example records permission decisions in tracing output, which is not durable.
// Production composition must inject its required audit storage implementation.
struct TracingExecutionAudit;
impl ExecutionAudit for TracingExecutionAudit {
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()> {
        Box::pin(async move {
            match record {
                ExecutionAuditRecord::QueueReordered(record) => {
                    tracing::info!(
                        session_id = %record.session_id().as_str(),
                        before = ?record.change().before(),
                        after = ?record.change().after(),
                        actor = ?record.actor(),
                        cause = ?record.cause(),
                        "queue reorder selected before local application"
                    );
                }
                ExecutionAuditRecord::Finished(record) => {
                    tracing::info!(session_id = %record.session_id().as_str(), execution_id = %record.execution_id().as_str(), result = ?record.result(), "execution released locally by runtime");
                }
                ExecutionAuditRecord::SessionClosed(record) => {
                    tracing::info!(session_id = %record.closure().session_id().as_str(), reason = ?record.closure().reason(), origin = ?record.origin(), "live session closed locally");
                }
                ExecutionAuditRecord::Cancelled(cancellation) => {
                    tracing::info!(
                        session = cancellation.session_id().as_str(),
                        execution = cancellation.request().execution_id().as_str(),
                        permission = cancellation.request().id().as_str(),
                        reason = cancellation_reason_code(cancellation.request().state()),
                        "Permission cancelled"
                    );
                }
                ExecutionAuditRecord::Answered(answer) => {
                    tracing::info!(
                        session = answer.session_id().as_str(),
                        execution = answer.resolution().request().execution_id().as_str(),
                        permission = answer.resolution().request().id().as_str(),
                        "Permission answer observed"
                    );
                }
                ExecutionAuditRecord::QuestionAnswered(answered) => {
                    tracing::info!(
                        session = answered.session_id().as_str(),
                        execution = answered.execution_id().as_str(),
                        question = answered.question_id().as_str(),
                        delivery = ?answered.delivery(),
                        "Agent question answered"
                    );
                }
                ExecutionAuditRecord::ReviewDeclined(declined) => {
                    tracing::info!(
                        session = declined.session_id().as_str(),
                        execution = declined.execution_id().as_str(),
                        tool = declined.decline().declared().unwrap_or("<unnamed>"),
                        reason = ?declined.decline().reason(),
                        delivery = ?declined.delivery(),
                        "Review declined without being offered"
                    );
                }
                ExecutionAuditRecord::Attachment(record) => {
                    tracing::info!(session = record.session_id().as_str(), before = ?record.before(), after = ?record.after(), cause = ?record.cause(), "Attachment lifecycle changed");
                }
                ExecutionAuditRecord::QueueAdmitted(record) => {
                    tracing::info!(session = record.session_id().as_str(), execution = record.execution_id().as_str(), mode = ?record.mode(), "Queue admission owned");
                }
                ExecutionAuditRecord::QueueSettled(record) => {
                    tracing::info!(session = record.session_id().as_str(), execution = record.execution_id().as_str(), mode = ?record.mode(), cause = ?record.cause(), "Queued input settled before dispatch");
                }
                ExecutionAuditRecord::SteeringAcknowledged(record) => {
                    tracing::info!(
                        session = record.session_id().as_str(),
                        execution = record.execution_id().as_str(),
                        target = record.target().as_str(),
                        "Native steering acknowledged"
                    );
                }
            }
            Ok(())
        })
    }
}
fn cancellation_reason_code(state: PermissionStateView<'_>) -> &str {
    match state {
        PermissionStateView::Cancelled { reason } => match reason.view() {
            PermissionCancellationReasonView::ProviderWithdrawal => "provider_withdrawal",
            PermissionCancellationReasonView::SessionClosed => "session_closed",
            PermissionCancellationReasonView::SessionFailed => "session_failed",
            PermissionCancellationReasonView::ExecutionFinished => "execution_finished",
            PermissionCancellationReasonView::ExecutionFailed => "execution_failed",
            PermissionCancellationReasonView::DeadlineExceeded => "deadline_exceeded",
            PermissionCancellationReasonView::EventConsumerDropped => "event_consumer_dropped",
            PermissionCancellationReasonView::SessionHandlesDropped => "session_handles_dropped",
            PermissionCancellationReasonView::Custom(reason) => reason.code(),
        },
        _ => "invalid_cancellation",
    }
}
fn close_action() -> ActionContext {
    ActionContext::new("nessa.smoke-host", "nessa.cli", "close-smoke-session").unwrap()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    if let Err(error) = run().await {
        tracing::error!(%error, "Claude example failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 8 {
        return Err("usage: claude_acp CATALOG NODE ACP_ENTRY WORKSPACE MODEL INPUT_TOKENS PROMPT MODE(text|close|close-write|deny-write|verify-write)".into());
    }
    let utf8 = |i: usize| args[i].to_str().ok_or("argument must be UTF-8");
    let model = ModelMetadata::try_from(
        load_catalog(File::open(&args[0])?)?.select("anthropic", utf8(4)?)?,
    )?;
    let mode = utf8(7)?;
    if !["text", "close", "close-write", "deny-write", "verify-write"].contains(&mode) {
        return Err("unknown mode".into());
    }
    // Composition chooses credential/environment sources. The adapter never reads
    // global environment or copies a credential from another namespace.
    let mut environment = BTreeMap::new();
    for key in [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "CLAUDE_CONFIG_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(key.into(), value);
        }
    }
    let mut credential_environment = BTreeMap::new();
    for key in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
        if let Some(value) = std::env::var_os(key) {
            credential_environment.insert(key.into(), value);
        }
    }
    let workspace = std::fs::canonicalize(PathBuf::from(&args[3]))?;
    // Admission window (input + reserved output), then per-response output cap.
    // These small smoke-test values are not model defaults or elapsed-time limits.
    let limits = TokenLimits::new(100_000, 1000)?;
    let audit: Arc<dyn ExecutionAudit> = Arc::new(TracingExecutionAudit);
    let binding = ClaudeAcpProvider::new(
        AcpConfig {
            executable: ExecutableUseSnapshot::unmanaged(PathBuf::from(&args[1])),
            arguments: vec![args[2].clone()],
            environment,
            credential_environment,
            workspace: workspace.clone(),
            tools_enabled: mode.ends_with("write"),
            mcp_servers: Vec::new(),
            permissions: PermissionOfferPolicy::once_only(),
            // The launch belongs to the operating system: a runtime written
            // by a fresh install is scanned on its first execution.
            launch_timeout: Duration::from_secs(120),
            startup_timeout: Duration::from_secs(45),
            execution_timeout: None,
            shutdown_grace: Duration::from_secs(3),
            kill_timeout: Duration::from_secs(2),
            event_capacity: 256,
            max_frame_bytes: 1024 * 1024,
            max_incoming_frame_bytes: 1024 * 1024,
            images: None,
            clock: Arc::new(RuntimeClock::new()),
        },
        &model,
        limits,
        audit.clone(),
    )?
    .with_system_prompt(
        SystemPromptBuilder::new()
            .text(
                PromptSource::new(PromptSourceKind::Core, "nessa/smoke")?,
                "You are Nessa, a helpful assistant. Follow the user's request.",
            )
            .build()?,
    );
    let storage = Arc::new(LocalFileStorage::new(workspace.join(".nessa/sessions"))?);
    let session = Agent::prepare(
        Arc::new(binding),
        SessionManager::open(Some(SessionId::new("smoke")?), storage).await?,
        audit,
    )
    .await?;
    let attachment = session.authorize_attachment(AttachmentRequest::CallerRequested(
        ActionContext::new("nessa.smoke-host", "nessa.cli", "attach-smoke-session")?,
    ))?;
    session.start_attachment(attachment)?.wait().await?;
    let mut events = session.subscribe();
    tracing::info!(
        model = session.capabilities().model().model_id(),
        "Binding ready"
    );
    let execution_id = ExecutionId::new(format!(
        "smoke-{}",
        session
            .session_manager()
            .snapshot()
            .await
            .expect("opened session")
            .invocations
            .len()
            + 1
    ))?;
    let input = ExecutionRequest {
        execution_id: execution_id.clone(),
        user_message: UserMessage::text_only(PromptText::new(utf8(6)?)?),
        estimated_input_tokens: utf8(5)?.parse()?,
        reserved_output_tokens: limits.max_output(),
    };
    let executing = session.clone();
    let action = ActionContext::new("nessa.smoke-host", "nessa.cli", execution_id.as_str())?;
    let pending = tokio::spawn(async move { executing.invoke(input, action).await });
    tokio::pin!(pending);
    let mut requested_close = false;
    let mut allowed = 0;
    let mut stream_failure = None;
    let outcome = loop {
        let event = tokio::select! {
            result = &mut pending => { break result?; }
            event = events.next() => event,
        };
        match event {
            Ok(Some(event)) => match event.into_update() {
                ExecutionUpdate::Message(chunk) if chunk.kind() == MessageKind::Text => {
                    let text = chunk.as_str();
                    tracing::info!(%text, "Agent text");
                    if mode == "close" && !requested_close && !text.is_empty() {
                        requested_close = true;
                        tracing::info!(cleanup = ?session.close(close_action()).await?, "Closed after text");
                    }
                }
                ExecutionUpdate::PermissionRequested {
                    id, input, options, ..
                } => {
                    if mode == "close-write" {
                        tracing::info!(cleanup = ?session.close(close_action()).await?, "Closed with pending permission");
                        continue;
                    }
                    let allow_once = mode == "verify-write"
                        && allowed == 0
                        && input.name == "Write"
                        && serde_json::from_str::<serde_json::Value>(&input.arguments_json)?
                            == serde_json::json!({
                                "file_path": workspace.join("nessa-binding-smoke.txt"),
                                "content": "Nessa binding smoke test\n"
                            });
                    let resolution = session
                        .answer_permission(PermissionAnswer {
                            attribution: attribution(id.as_str()),
                            execution_id: execution_id.clone(),
                            id: id.clone(),
                            option_id: options
                                .choices()
                                .iter()
                                .find(|option| {
                                    option.decision().clone()
                                        == if allow_once {
                                            PermissionDecision::new(
                                                PermissionEffect::Allow,
                                                PermissionScope::request(),
                                            )
                                        } else {
                                            PermissionDecision::new(
                                                PermissionEffect::Deny,
                                                PermissionScope::request(),
                                            )
                                        }
                                })
                                .ok_or("required once-only permission option missing")?
                                .id()
                                .clone(),
                        })
                        .await?;
                    if allow_once {
                        allowed += 1;
                    }
                    tracing::info!(
                        principal = resolution.attribution().actor().principal_id(),
                        request = resolution.attribution().actor().request_id(),
                        basis = ?resolution.attribution().basis(),
                        allow_once,
                        "Permission answered"
                    );
                }
                ExecutionUpdate::Finished(_) => break pending.as_mut().await?,
                _ => {}
            },
            Ok(None) => break pending.as_mut().await?,
            Err(error) => {
                stream_failure = Some(error);
                break pending.as_mut().await?;
            }
        }
    };
    let cleanup = session.close(close_action()).await;
    tracing::info!(
        ?outcome,
        ?cleanup,
        exact_writes_allowed = allowed,
        "Execution finished"
    );
    outcome?;
    cleanup?;
    if let Some(error) = stream_failure {
        return Err(error.into());
    }
    Ok(())
}

fn attribution(permission_id: &str) -> ApprovalAttribution {
    ApprovalAttribution::new(
        ActionContext::new(
            "nessa.smoke-runner",
            "nessa.cli",
            format!("smoke-answer-{permission_id}"),
        )
        .unwrap(),
        ApprovalBasis::Mode(ApprovalModeSnapshot::new("smoke-file-policy", "1").unwrap()),
    )
}
