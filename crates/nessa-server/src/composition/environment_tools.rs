//! The agent's environment tools as a server the gateway serves itself
//! (issue #700): `environments_list` and `run`, over the conversation
//! service, offered to a conversation's harness beside its configured MCP
//! servers and reached through the same relay.
//!
//! ```text
//! harness ──tools/call──▶ stand-in ──▶ Relay ──▶ EnvironmentTools::call
//!   environments_list ──▶ ConversationService::list_environments
//!   run ──▶ its arguments read ──▶ ConversationService::run_command ──▶ its record
//! ```
//!
//! Arrows are calls, in order. A tool's answer is MCP's `CallToolResult`:
//! what it came to as text for the model, and the same as
//! `structuredContent`. The service is attached once composed, after the
//! relay that needs this server; a call before then is answered as an error.
use crate::conversation::application::{
    CommandAnswer, CommandCall, CommandCallError, CommandResult, ConversationService,
    EnvironmentListing,
};
use crate::conversation::domain::CommandPolicy;
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandRefusal, CommandWork, LeaseCleanup, LeaseEndCause, LeaseRefusal,
    SandboxProfile,
};
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;

/// The server's name, as the harness sees it.
pub(super) const SERVER_NAME: &str = "nessa-environments";
/// What changes when the tools' names, arguments or meaning do.
const TOOLS_REVISION: &str = "environment-tools/1";

/// `environments_list` and `run`, served over a conversation service.
pub(super) struct EnvironmentTools {
    policy: Vec<String>,
    service: OnceLock<ConversationService>,
}

impl EnvironmentTools {
    /// The tools for `policy`, with no service yet.
    pub(super) fn new(policy: &CommandPolicy) -> Arc<Self> {
        Arc::new(Self {
            policy: policy.describe(),
            service: OnceLock::new(),
        })
    }

    /// Serve calls with `service` from now on. Attached once; a second is
    /// ignored.
    pub(super) fn attach(&self, service: ConversationService) {
        let _ = self.service.set(service);
    }
}

impl BuiltInServer for EnvironmentTools {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn configuration(&self) -> Vec<String> {
        let mut configuration = vec![TOOLS_REVISION.to_owned()];
        configuration.extend(self.policy.iter().cloned());
        configuration
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            json!({
                "name": "environments_list",
                "title": "List environments",
                "description": "List where this conversation could work: `here`, this gateway's own machine, and each SSH host it is configured with. Says which one the agent runs in, which ones `run` may run commands on, and whether a connection to each is open now. Reaches nothing to answer.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
                "annotations": {"readOnlyHint": true, "openWorldHint": false},
            }),
            json!({
                "name": "run",
                "title": "Run a command on an SSH host",
                "description": format!(
                    "Run one program once on an SSH host that environments_list says takes commands, in that host's workspace, as the account it serves as, with nothing enclosing it. `argv` is the program and its arguments exactly; no shell reads them. The command is stopped when its timeout passes or this call is cancelled. Answers its exit and the last {} bytes of each of its output streams. At most {} commands run at once in a conversation.",
                    nessa_sdk::domain::agent_execution::leases::CommandOutput::MAX_TAIL_BYTES,
                    nessa_sdk::domain::agent_execution::leases::Lease::MAX_LIVE_COMMANDS,
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "environment": {"type": "string", "description": "The SSH host, as environments_list names it."},
                        "argv": {
                            "type": "array",
                            "items": {"type": "string"},
                            "minItems": 1,
                            "maxItems": CommandWork::MAX_ARGS,
                            "description": "The program, then its arguments.",
                        },
                        "cwd": {"type": "string", "description": "A directory beneath the host's workspace to run in; the workspace when absent."},
                        "timeoutMs": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": CommandWork::MAX_TIMEOUT_MS,
                            "description": format!("How long it may run, in milliseconds; {} when absent.", CommandWork::DEFAULT_TIMEOUT_MS),
                        },
                        "sandbox": {
                            "type": "string",
                            "enum": ["none"],
                            "description": "What encloses it. Only `none` can be had: nothing does.",
                        },
                    },
                    "required": ["environment", "argv"],
                    "additionalProperties": false,
                },
                "annotations": {"destructiveHint": true, "openWorldHint": true},
            }),
        ]
    }

    fn call(
        &self,
        session: &SessionId,
        call: String,
        tool: String,
        arguments: Value,
        stop: watch::Receiver<bool>,
    ) -> BuiltInFuture {
        let service = self.service.get().cloned();
        let conversation = ConversationId::new(session.as_str());
        Box::pin(async move {
            let (Some(service), Ok(conversation)) = (service, conversation) else {
                return failed("this conversation's environment tools are not available now");
            };
            match tool.as_str() {
                "environments_list" => match service.list_environments(&conversation).await {
                    Ok(listed) => listing(&listed),
                    Err(error) => failed(&error.to_string()),
                },
                "run" => {
                    let call = match command_call(arguments, call) {
                        Ok(call) => call,
                        Err(problem) => return failed(&problem),
                    };
                    match service.run_command(&conversation, call, stop).await {
                        Ok(answer) => answered(&answer),
                        Err(error) => failed(call_error(&error)),
                    }
                }
                _ => failed("no such tool"),
            }
        })
    }
}

/// `run`'s arguments, as its input schema says them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RunArguments {
    environment: String,
    argv: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    sandbox: Option<String>,
}

fn command_call(arguments: Value, call: String) -> Result<CommandCall, String> {
    let arguments: RunArguments =
        serde_json::from_value(arguments).map_err(|error| format!("run's arguments: {error}"))?;
    let sandbox = match arguments.sandbox.as_deref() {
        None | Some("none") => SandboxProfile::None,
        Some("harness-default") => SandboxProfile::HarnessDefault,
        Some(other) => return Err(format!("run's arguments: no sandbox is named {other:?}")),
    };
    Ok(CommandCall {
        environment: arguments.environment,
        argv: arguments.argv,
        cwd: arguments.cwd,
        timeout_ms: arguments
            .timeout_ms
            .unwrap_or(CommandWork::DEFAULT_TIMEOUT_MS),
        sandbox,
        call,
    })
}

fn listing(listed: &[EnvironmentListing]) -> Value {
    let environments: Vec<Value> = listed
        .iter()
        .map(|environment| {
            json!({
                "name": environment.name,
                "here": environment.here,
                "reachable": environment.reachable,
                "commands": environment.commands,
                "current": environment.current,
            })
        })
        .collect();
    result(json!({"environments": environments}), false)
}

fn answered(answer: &CommandAnswer) -> Value {
    match answer {
        CommandAnswer::Refused(refusal) => result(
            json!({"refused": refusal_code(*refusal), "message": refusal_message(*refusal)}),
            true,
        ),
        CommandAnswer::Ran {
            lease,
            result: ran,
            recorded,
        } => {
            // A success is an exit 0 whose end is recorded here and whose
            // cleanup the host confirmed; anything less is said as an error.
            let succeeded =
                ran.exit == CommandExit::Exited { code: 0 } && *recorded && ran.cleanup.is_some();
            result(
                json!({
                    "lease": lease.as_str(),
                    "exit": exit(ran.exit),
                    "stdout": String::from_utf8_lossy(&ran.stdout),
                    "stderr": String::from_utf8_lossy(&ran.stderr),
                    "droppedBytes": ran.dropped_bytes,
                    "cleanup": cleanup(ran),
                    "recorded": recorded,
                }),
                !succeeded,
            )
        }
    }
}

fn exit(exit: CommandExit) -> Value {
    match exit {
        CommandExit::Exited { code } => json!({"kind": "exited", "code": code}),
        CommandExit::Signalled { signal } => json!({"kind": "signalled", "signal": signal}),
        CommandExit::TimedOut => json!({"kind": "timed_out"}),
        CommandExit::Stopped { cause } => json!({"kind": "stopped", "cause": cause_code(cause)}),
        CommandExit::NotStarted => json!({"kind": "not_started"}),
        CommandExit::Unanswered => json!({"kind": "unanswered"}),
    }
}

fn cleanup(ran: &CommandResult) -> &'static str {
    match ran.cleanup {
        Some(LeaseCleanup::Confirmed { forced: false }) => "confirmed",
        Some(LeaseCleanup::Confirmed { forced: true }) => "forced",
        Some(LeaseCleanup::NotHeld) => "not_held",
        None => "uncertain",
    }
}

fn cause_code(cause: LeaseEndCause) -> &'static str {
    match cause {
        LeaseEndCause::Stopped => "stopped",
        LeaseEndCause::Closed => "closed",
        LeaseEndCause::Revoked => "revoked",
        LeaseEndCause::Expired => "expired",
        LeaseEndCause::Lost => "lost",
    }
}

fn refusal_code(refusal: CommandRefusal) -> &'static str {
    match refusal {
        CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable) => "sandbox_unavailable",
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable) => {
            "environment_unreachable"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentVersionMismatch) => {
            "environment_version_mismatch"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentBusy) => "environment_busy",
        CommandRefusal::Environment(LeaseRefusal::AgentUnavailable) => "agent_unavailable",
        CommandRefusal::EnvironmentNotGranted => "environment_not_granted",
        CommandRefusal::CommandDenied => "command_denied",
        CommandRefusal::BudgetExceeded => "budget_exceeded",
        CommandRefusal::CommandsUnavailable => "commands_unavailable",
    }
}

fn refusal_message(refusal: CommandRefusal) -> &'static str {
    match refusal {
        CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable) => {
            "a command runs with sandbox `none` only"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable) => {
            "the host could not be reached, or did not answer in time"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentVersionMismatch) => {
            "the host runs another build of Nessa"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentBusy) => {
            "the host is serving another gateway"
        }
        CommandRefusal::Environment(LeaseRefusal::AgentUnavailable) => "the host cannot run this",
        CommandRefusal::EnvironmentNotGranted => {
            "commands are not granted on that environment; see environments_list"
        }
        CommandRefusal::CommandDenied => "this gateway's tool policy does not allow that program",
        CommandRefusal::BudgetExceeded => {
            "as many commands as a conversation may run at once are running"
        }
        CommandRefusal::CommandsUnavailable => "that environment runs no commands",
    }
}

fn call_error(error: &CommandCallError) -> &str {
    match error {
        CommandCallError::Invalid(problem) => problem,
        CommandCallError::NoTurn => "no turn of this conversation is running to run it for",
        CommandCallError::Unrecorded => "the command could not be recorded, so it did not run",
        CommandCallError::NotConfigured => "this gateway grants no commands",
        CommandCallError::Cancelled => "the call was cancelled before the command was asked for",
    }
}

fn result(structured: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&structured).unwrap_or_default();
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
        "isError": is_error,
    })
}

fn failed(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

#[cfg(test)]
#[path = "../../tests/composition/environment_tools.rs"]
mod tests;
