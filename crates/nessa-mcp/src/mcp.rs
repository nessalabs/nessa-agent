//! Bounded stdio MCP transport. One command at a time; cancellation never drops its owner.
//! Original tool arguments are approved by the ACP client before Claude invokes MCP.
use crate::shell::{
    application::{RunRequest, RunResult, ShellError, ShellService},
    domain::{RunCause, ShellCommand, StopCause, ToolInvocation, ToolRequestId},
};
use serde::Deserialize;
use serde_json::{json, value::RawValue, Value};
use std::fmt;
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
    task: JoinHandle<Completion>,
}
/// What one admitted command finished with.
///
/// The reply is one fact; whether this server verified the command's process
/// cleanup and whether any of its audit evidence was accepted are two others,
/// and a command that ends at EOF or on SIGTERM has no reader left for the
/// reply. They are carried separately so the last two survive a shutdown that
/// discards the first. A command's own non-zero exit is not this server failing.
struct Completion {
    response: Value,
    cleanup_verified: bool,
    /// An audit delivery that was not acknowledged, at whichever phase:
    /// admission, which no runner reaches, as well as start and completion.
    audit_error: Option<String>,
}
/// A command that stopped without confirming everything it owed.
///
/// The two facts stay apart here, rather than being joined into a sentence, so
/// that a caller can branch on which one held instead of reading the prose.
#[derive(Debug)]
pub struct UncleanStop {
    /// This server could not confirm the command's processes were cleaned up.
    pub cleanup_unverified: bool,
    /// An audit delivery this server could not get acknowledged.
    pub audit_error: Option<String>,
}
impl fmt::Display for UncleanStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut reasons = Vec::new();
        if self.cleanup_unverified {
            reasons.push("could not verify process cleanup".to_owned());
        }
        if let Some(error) = &self.audit_error {
            reasons.push(format!("audit was not accepted: {error}"));
        }
        write!(f, "final command {}", reasons.join("; "))
    }
}
impl std::error::Error for UncleanStop {}

impl Completion {
    /// Why this command was not a clean stop, keeping every reason.
    ///
    /// Cleanup and audit delivery are separate facts and a command can fail
    /// both; reporting only the first one found would drop the other. `None`
    /// means the command stopped cleanly, whatever its own exit status was.
    fn unclean(&self) -> Option<UncleanStop> {
        let unclean = UncleanStop {
            cleanup_unverified: !self.cleanup_verified,
            audit_error: self.audit_error.clone(),
        };
        (unclean.cleanup_unverified || unclean.audit_error.is_some()).then_some(unclean)
    }
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
                let completion = result?;
                // The client is still here, so the reply is where this command's
                // outcome is reported. A command that ran says so through
                // `tool_result`, which marks the call an error and carries both
                // `cleanupVerified` and `auditError`; one refused at admission
                // has no such fields and says it in its message.
                //
                // Unverified cleanup still ends the session, as it always has,
                // and `main` fails the process over any scope the supervisor
                // cannot verify on its way down — that check covers cleanup, not
                // audit. So an audit delivery this server could not get
                // acknowledged is reported to the client and to the log and
                // deliberately does not change the exit code while someone is
                // there to read it. Only the paths with no reader left (see
                // `finish`) have nothing but the exit code to say it with.
                let unclean = completion.unclean();
                let verified = completion.cleanup_verified;
                write(completion.response).await?;
                if let Some(reason) = unclean { tracing::error!(%reason, "command did not stop cleanly"); }
                if !verified { break; }
            }
            read = bounded_input.read_until(b'\n', &mut frame) => {
                match read {
                    Ok(0) => { finish(active, StopCause::ClientClosed).await?; break; }
                    Err(error) => { return Err(finish_after(active, StopCause::ClientClosed, error.into()).await); }
                    Ok(_) if frame.len() as u64 > MAX_FRAME => { return Err(finish_after(active, StopCause::ClientClosed, "MCP frame exceeds 64 KiB".into()).await); }
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
                                        let outcome = service.run(&request, receive).await;
                                        completion(reply_id, &command_id, outcome)
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
                if let Err(error) = write(response).await { return Err(finish_after(active, StopCause::ClientClosed, error).await); }
            }
        }
    }
    Ok(())
}
/// Stop the admitted command, if any, and keep what it finished with.
///
/// Its reply has nowhere to go once the client is gone, so the exit code is all
/// that is left to carry the outcome: a command that could not verify its
/// process cleanup, or whose audit was not accepted, must not leave this process
/// reporting success. What counts as unclean is [`Completion::unclean`], the same
/// answer the still-connected path reports through the reply.
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
        if let Some(reason) = a.task.await?.unclean() {
            return Err(reason.into());
        }
    }
    Ok(())
}

/// Stop the admitted command while keeping `cause`, which is why the server is
/// stopping, ahead of anything the stop itself reports.
///
/// The read error or protocol violation that ended the loop is the diagnosis; a
/// command that then failed to stop cleanly is a second fact, logged rather than
/// substituted for the first.
async fn finish_after(active: &mut Option<Active>, stop: StopCause, cause: Error) -> Error {
    if let Err(error) = finish(active, stop).await {
        tracing::error!(%error, "the command running at shutdown did not stop cleanly");
    }
    cause
}
/// What one command's outcome means for this server, as the two facts that
/// outlive the reply plus the reply itself.
///
/// A `ShellError` is a refusal to admit rather than a command that ran and
/// failed: nothing started, so there is no process scope left to verify. But
/// admission is refused precisely because its audit was not acknowledged, and
/// dropping that here would turn an unaudited command into a clean stop — the
/// same successful-looking shutdown these facts are carried separately to
/// prevent. Matched by variant so a future refusal that is not an audit failure
/// has to say what it is rather than inheriting this one's meaning.
fn completion(
    reply_id: Value,
    command_id: &str,
    outcome: Result<RunResult, ShellError>,
) -> Completion {
    match outcome {
        Ok(result) => Completion {
            cleanup_verified: result.cleanup_verified,
            audit_error: result.audit_error.clone(),
            response: success(reply_id, tool_result(command_id, result)),
        },
        Err(error) => {
            let message = error.to_string();
            Completion {
                response: success(
                    reply_id,
                    json!({"isError":true,"content":[{"type":"text","text":format!("commandId={command_id}: {message}")}]}),
                ),
                cleanup_verified: true,
                audit_error: match error {
                    ShellError::AdmissionAuditFailed => Some(message),
                },
            }
        }
    }
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

// A directory, because a file directly under `tests/` would also be picked up as
// an integration target; `tests/stdio.rs` is this crate's only one.
#[cfg(test)]
#[path = "../tests/mcp/shutdown.rs"]
mod tests;
