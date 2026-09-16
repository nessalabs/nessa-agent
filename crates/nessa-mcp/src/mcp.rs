//! Bounded stdio MCP transport. One command at a time; cancellation never drops its owner.
//! Original tool arguments are approved by the ACP client before Claude invokes MCP.
use crate::shell::{
    application::{RunRequest, RunResult, ShellService},
    domain::{RunCause, ShellCommand, StopCause, ToolInvocation, ToolRequestId},
};
use serde::Deserialize;
use serde_json::{json, value::RawValue, Value};
use std::{path::PathBuf, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    sync::watch,
    task::JoinHandle,
};
use uuid::Uuid;

type Error = Box<dyn std::error::Error>;
const MAX_FRAME: u64 = 65536;
#[derive(Deserialize)]
struct Request {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Box<RawValue>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    arguments: Arguments,
    #[serde(rename = "_meta")]
    _meta: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Arguments {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
}
fn default_timeout() -> u64 {
    120
}
fn tool_request_id(id: &Value) -> Option<ToolRequestId> {
    if let Some(value) = id.as_i64() {
        Some(ToolRequestId::Signed(value))
    } else if let Some(value) = id.as_u64() {
        Some(ToolRequestId::Unsigned(value))
    } else {
        id.as_str()
            .map(|value| ToolRequestId::Text(value.to_owned().into_boxed_str()))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cancel {
    request_id: Value,
}
struct Active {
    id: Value,
    stop: watch::Sender<Option<StopCause>>,
    task: JoinHandle<(Value, bool)>,
}

pub async fn serve(
    service: Arc<ShellService>,
    workspace: PathBuf,
    owner: String,
) -> Result<(), Error> {
    let mut active = None;
    let result = serve_loop(service, workspace, owner, &mut active).await;
    let cleanup = finish(&mut active, StopCause::ClientClosed).await;
    result.and(cleanup)
}
async fn serve_loop(
    service: Arc<ShellService>,
    workspace: PathBuf,
    owner: String,
    active: &mut Option<Active>,
) -> Result<(), Error> {
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut initialized = false;
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut frame = Vec::new();
    loop {
        let mut bounded_input = (&mut stdin).take(MAX_FRAME + 1 - frame.len() as u64);
        tokio::select! {
            biased;
            _ = signal.recv() => { finish(active, StopCause::HostShutdown).await?; break; }
            _ = tokio::signal::ctrl_c() => { finish(active, StopCause::HostShutdown).await?; break; }
            result = async { match active.as_mut() { Some(a) => (&mut a.task).await, None => std::future::pending().await } } => {
                *active = None;
                let (response, reusable) = result?;
                write(response).await?;
                if !reusable { break; }
            }
            read = bounded_input.read_until(b'\n', &mut frame) => {
                match read {
                    Ok(0) => { finish(active, StopCause::ClientClosed).await?; break; }
                    Err(error) => { finish(active, StopCause::ClientClosed).await?; return Err(error.into()); }
                    Ok(_) if frame.len() as u64 > MAX_FRAME => { finish(active, StopCause::ClientClosed).await?; return Err("MCP frame exceeds 64 KiB".into()); }
                    Ok(_) => {}
                }
                let received = std::mem::take(&mut frame);
                let request: Request = match serde_json::from_slice(&received) { Ok(r) => r, Err(_) => { write(error(Value::Null, -32700, "invalid JSON-RPC request")).await?; continue; } };
                let valid_id = request.id.as_ref().is_none_or(|id| id.is_i64() || id.is_u64() || id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 256));
                if request.jsonrpc != "2.0" || !valid_id { write(error(Value::Null, -32600, "invalid JSON-RPC envelope")).await?; continue; }
                if request.method == "notifications/cancelled" {
                    if let Some(params) = request.params {
                        if let Ok(cancel) = serde_json::from_str::<Cancel>(params.get()) {
                            if let Some(a) = active.as_ref().filter(|a| a.id == cancel.request_id) { a.stop.send_if_modified(|cause| { if cause.is_none() { *cause = Some(StopCause::ClientCancelled); true } else { false } }); }
                        }
                    }
                    continue;
                }
                let Some(id) = request.id else { continue; };
                let response = match request.method.as_str() {
                    "initialize" if !initialized => { initialized = true; success(id, json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"nessa-mcp","version":env!("CARGO_PKG_VERSION")},"instructions":"Use shell for all command execution. Commands run in the configured workspace and have bounded output. Background descendants are cleaned up when a command finishes. Each result has a tracked commandId."})) }
                    "ping" => success(id, json!({})),
                    _ if !initialized => error(id, -32002, "initialize first"),
                    "tools/list" => success(id, json!({"tools":[{"name":"shell","description":"Run a Bash command. Returns commandId, process identity, stdout/stderr, exit code, timeout/cancellation cause, and verified cleanup status","inputSchema":{"type":"object","properties":{"command":{"type":"string","minLength":1,"maxLength":32768},"timeoutSeconds":{"type":"integer","minimum":1,"maximum":3600,"default":120}},"required":["command"],"additionalProperties":false},"annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true}}]})),
                    "tools/call" if active.is_some() => error(id, -32000, "a command is already running; wait for its result"),
                    "tools/call" => {
                        let call = request.params.as_ref().and_then(|p| serde_json::from_str::<Call>(p.get()).ok());
                        match call.filter(|c| c.name == "shell") {
                            Some(call) => match ShellCommand::new(call.arguments.command, call.arguments.timeout_seconds) {
                                Ok(command) => {
                                    let invocation = ToolInvocation::provider(
                                        owner.clone(),
                                        tool_request_id(&id).expect("validated JSON-RPC request identity"),
                                    ).expect("bounded composition and request identities");
                                    let request = RunRequest { id: Uuid::new_v4().to_string(), invocation, command, cwd: workspace.clone() };
                                    let service = service.clone(); let reply_id = id.clone();
                                    let (stop, receive) = watch::channel(None);
                                    let task = tokio::spawn(async move {
                                        let command_id = request.id.clone();
                                        match service.run(&request, receive).await {
                                            Ok(result) => { let reusable = result.cleanup_verified; (success(reply_id, tool_result(&command_id, result)), reusable) },
                                            Err(message) => (success(reply_id, json!({"isError":true,"content":[{"type":"text","text":format!("commandId={command_id}: {message}")}]})), true),
                                        }
                                    });
                                    *active = Some(Active { id, stop, task });
                                    continue;
                                }
                                Err(message) => {
                                    let message = message.to_string();
                                    error(id, -32602, &message)
                                }
                            },
                            None => error(id, -32602, "expected shell with command and optional timeoutSeconds"),
                        }
                    }
                    _ => error(id, -32601, "method not supported"),
                };
                if let Err(error) = write(response).await { finish(active, StopCause::ClientClosed).await?; return Err(error); }
            }
        }
    }
    Ok(())
}
async fn finish(active: &mut Option<Active>, cause: StopCause) -> Result<(), Error> {
    if let Some(a) = active.take() {
        a.stop.send_if_modified(|current| {
            if current.is_none() {
                *current = Some(cause);
                true
            } else {
                false
            }
        });
        // Do not abort: the admitted operation must perform cleanup and commit final evidence.
        a.task.await?;
    }
    Ok(())
}
fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
async fn write(value: Value) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    let mut stdout = tokio::io::stdout();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        stdout.write_all(&bytes).await?;
        stdout.flush().await
    })
    .await??;
    Ok(())
}
fn tool_result(id: &str, result: RunResult) -> Value {
    let failed = result.cause != RunCause::Exited
        || result.exit_code != Some(0)
        || !result.cleanup_verified
        || !result.output_errors.is_empty()
        || result.audit_error.is_some();
    let output = json!({"commandId":id,"cause":format!("{:?}",result.cause),"scopeId":result.scope_id,"processId":result.process_id,"osPid":result.os_pid,"exitCode":result.exit_code,"exitSignal":result.exit_signal,"forced":result.forced,"stdout":result.stdout,"stderr":result.stderr,"droppedBytes":result.dropped_bytes,"cleanupVerified":result.cleanup_verified,"cleanupError":result.cleanup_error,"outputErrors":result.output_errors,"auditError":result.audit_error});
    json!({"isError":failed,"content":[{"type":"text","text":output.to_string()}]})
}
