//! Explicit host composition for a local smoke run. Permissions default to deny.
use nessa_sdk::domain::agent_execution::value_objects::*;
use nessa_sdk::{
    application::agent_binding::{Agent, AgentBinding, BindingUpdate, PermissionAnswer, Prompt},
    domain::{common::value_objects::TokenLimits, model_metadata::entities::ModelMetadata},
    infrastructure::{
        claude_acp::{ClaudeAcpBinding, ClaudeAcpConfig},
        model_metadata_json::load_catalog,
    },
};
use std::{collections::BTreeMap, error::Error, fs::File, path::PathBuf, time::Duration};

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
        return Err("usage: claude_acp CATALOG NODE ACP_ENTRY WORKSPACE MODEL INPUT_TOKENS PROMPT MODE(text|stop|stop-write|deny-write|verify-write)".into());
    }
    let utf8 = |i: usize| args[i].to_str().ok_or("argument must be UTF-8");
    let model = ModelMetadata::try_from(
        load_catalog(File::open(&args[0])?)?.select("anthropic", utf8(4)?)?,
    )?;
    let mode = utf8(7)?;
    if !["text", "stop", "stop-write", "deny-write", "verify-write"].contains(&mode) {
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
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(key.into(), value);
        }
    }
    let workspace = std::fs::canonicalize(PathBuf::from(&args[3]))?;
    // Admission window (input + reserved output), then per-response output cap.
    // These small smoke-test values are not model defaults or elapsed-time limits.
    let limits = TokenLimits::new(100_000, 1000)?;
    let binding = ClaudeAcpBinding::new(
        ClaudeAcpConfig {
            executable: PathBuf::from(&args[1]),
            arguments: vec![args[2].clone()],
            environment,
            workspace: workspace.clone(),
            file_tools: mode.ends_with("write"),
            startup_timeout: Duration::from_secs(45),
            prompt_timeout: None,
            shutdown_grace: Duration::from_secs(3),
            kill_timeout: Duration::from_secs(2),
            event_capacity: 256,
            max_frame_bytes: 1024 * 1024,
        },
        &model,
        limits,
    )?;
    let mut opened = binding.open().await?;
    tracing::info!(
        model = opened.capabilities.model().model_id(),
        "Binding ready"
    );
    let agent = std::sync::Arc::new(Agent::new(opened.session, opened.capabilities));
    let input = Prompt {
        execution_id: "smoke".into(),
        text: utf8(6)?.into(),
        input_tokens: utf8(5)?.parse()?,
        reserved_output_tokens: limits.max_output(),
    };
    let executing = agent.clone();
    let pending = tokio::spawn(async move { executing.prompt(input).await });
    tokio::pin!(pending);
    let mut requested_stop = false;
    let mut allowed = 0;
    let mut resolved = None;
    let mut stream_failure = None;
    let outcome = loop {
        let event = tokio::select! {
            result = &mut pending, if resolved.is_none() => {
                resolved = Some(result?);
                // This example opens one binding for one prompt. Close it so
                // even an admission rejection has a finite stream to drain.
                let _ = agent.stop().await;
                continue;
            }
            event = opened.events.next() => event,
        };
        match event {
            Ok(Some(event)) => match event {
                nessa_sdk::application::agent_binding::BindingEvent {
                    update: BindingUpdate::Message(MessageChunk::Text(text)),
                    ..
                } => {
                    tracing::info!(%text, "Agent text");
                    if mode == "stop" && !requested_stop && !text.is_empty() {
                        requested_stop = true;
                        tracing::info!(cleanup = ?agent.stop().await?, "Stopped after text");
                    }
                }
                nessa_sdk::application::agent_binding::BindingEvent {
                    update: BindingUpdate::PermissionRequested { id, input, .. },
                    ..
                } => {
                    if mode == "stop-write" {
                        tracing::info!(cleanup = ?agent.stop().await?, "Stopped with pending permission");
                        continue;
                    }
                    let allow_once = mode == "verify-write"
                        && allowed == 0
                        && matches!(*input,
                        FileToolInput::Write {ref path,ref content} if std::path::Path::new(path.as_str()) == workspace.join("nessa-binding-smoke.txt") && content == "Nessa binding smoke test\n");
                    agent
                        .answer_permission(PermissionAnswer {
                            execution_id: "smoke".into(),
                            id,
                            allow_once,
                        })
                        .await?;
                    if allow_once {
                        allowed += 1;
                    }
                    tracing::info!(allow_once, "Permission answered");
                }
                nessa_sdk::application::agent_binding::BindingEvent {
                    update: BindingUpdate::Finished(_),
                    ..
                } => {
                    break match resolved.take() {
                        Some(result) => result,
                        None => pending.as_mut().await?,
                    }
                }
                _ => {}
            },
            Ok(None) => {
                break match resolved.take() {
                    Some(result) => result,
                    None => pending.as_mut().await?,
                }
            }
            Err(error) => {
                stream_failure = Some(error);
                break match resolved.take() {
                    Some(result) => result,
                    None => pending.as_mut().await?,
                };
            }
        }
    };
    let cleanup = agent.stop().await;
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
