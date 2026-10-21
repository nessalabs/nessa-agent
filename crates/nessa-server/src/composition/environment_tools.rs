//! The agent's environment tools as a server the gateway serves itself
//! (issues #700, #701): `environments_list`, `run`, `artifacts_publish` and
//! `artifacts_fetch`, over the conversation service, offered to a conversation's harness beside
//! its configured MCP servers and reached through the same relay.
//!
//! ```text
//! harness ──tools/call──▶ stand-in ──▶ Relay ──▶ EnvironmentTools::call
//!   environments_list ──▶ ConversationService::list_environments
//!   run ──▶ its arguments read ──▶ ConversationService::run_command ──▶ its record
//!   artifacts_publish ──▶ its arguments read ──▶ ConversationService::publish_artifact
//!     ──▶ held | alreadyHeld, or refused by code
//!   artifacts_fetch ──▶ its arguments read ──▶ ConversationService::fetch_artifact
//!     ──▶ placed | alreadyPlaced, or refused by code
//! ```
//!
//! Arrows are calls, in order. A tool's answer is MCP's `CallToolResult`:
//! what it came to as text for the model, and the same as
//! `structuredContent`. The service is attached once composed, after the
//! relay that needs this server; a call before then is answered as an error.
use crate::conversation::application::{
    CommandAnswer, CommandCall, CommandCallError, CommandPublication, CommandResult,
    ConversationService, EnvironmentListing, FetchAnswer, FetchCall, FetchCallRefusal,
    PublicationOutcome, PublishAnswer, PublishCall, PublishCallRefusal,
};
use crate::conversation::domain::CommandPolicy;
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::lease::{CollectionRefusal, FetchRefusal, PublishRefusal, MAX_ARTIFACT_BYTES};
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
const TOOLS_REVISION: &str = "environment-tools/5";

/// `environments_list`, `run`, `artifacts_publish` and `artifacts_fetch`,
/// served over a conversation service.
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
                "description": "List where this conversation could work: `here`, this gateway's own machine, and each SSH host or paired environment this turn may use. Says which one the agent runs in, which ones `run` may run commands on for whoever started this turn, and whether a connection to each is open now. Reaches nothing to answer.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
                "annotations": {"readOnlyHint": true, "openWorldHint": false},
            }),
            json!({
                "name": "run",
                "title": "Run a command on an SSH host",
                "description": format!(
                    "Run one program once on an SSH host that environments_list says takes commands, in that host's workspace, as the account it serves as, with nothing enclosing it. `argv` is the program and its arguments exactly; no shell reads them. The command is stopped when its timeout passes or this call is cancelled. Answers its exit, the last {} bytes of each of its output streams, and each file it published. At most {} commands run at once in a conversation.",
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
            json!({
                "name": "artifacts_publish",
                "title": "Publish a file to the conversation",
                "description": format!(
                    "Publish one file from the workspace where this conversation's agent runs, so the person can see and save it in the conversation. `environment` must be where the agent runs now: the environment environments_list marks current. The file is copied as it is now, checked by its digest, and kept; at most {MAX_ARTIFACT_BYTES} bytes. Answers whether it is held, with its digest and size, or why it is not."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "environment": {"type": "string", "description": "Where the agent runs, as environments_list names it."},
                        "path": {"type": "string", "description": "The file: absolute, or relative to that environment's workspace. It must be inside the workspace."},
                        "mediaType": {"type": "string", "description": "Its media type, lowercase type/subtype; read from its extension when absent."},
                    },
                    "required": ["environment", "path"],
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
            }),
            json!({
                "name": "artifacts_fetch",
                "title": "Fetch a conversation file into the workspace",
                "description": format!(
                    "Place one file this conversation holds — one the person attached or one published to it — in the workspace where this conversation's agent runs. Name it exactly by its digest, media type and size. `environment` must be where the agent runs now: the environment environments_list marks current. The file is written to `destination`, checked by its digest, and placed only where nothing is: an existing file is never replaced. At most {MAX_ARTIFACT_BYTES} bytes. Answers where it is, or why it is not placed."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "environment": {"type": "string", "description": "Where the agent runs, as environments_list names it."},
                        "digest": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "The file's SHA-256, lowercase hex."},
                        "mediaType": {"type": "string", "description": "Its media type, lowercase type/subtype."},
                        "size": {"type": "integer", "minimum": 1, "maximum": MAX_ARTIFACT_BYTES, "description": "Its length in bytes."},
                        "destination": {"type": "string", "description": "Where to place it: absolute, or relative to that environment's workspace. It must be inside the workspace, in a directory that exists, where nothing is yet."},
                    },
                    "required": ["environment", "digest", "mediaType", "size", "destination"],
                    "additionalProperties": false,
                },
                "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false},
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
            let result: Value = async move {
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
                    "artifacts_publish" => {
                        let call = match publish_call(arguments, call) {
                            Ok(call) => call,
                            Err(problem) => return failed(&problem),
                        };
                        published(&service.publish_artifact(&conversation, call, stop).await)
                    }
                    "artifacts_fetch" => {
                        let call = match fetch_call(arguments, call) {
                            Ok(call) => call,
                            Err(problem) => return failed(&problem),
                        };
                        fetched(&service.fetch_artifact(&conversation, call, stop).await)
                    }
                    _ => failed("no such tool"),
                }
            }
            .await;
            result.into()
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

/// `artifacts_publish`'s arguments, as its input schema says them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PublishArguments {
    environment: String,
    path: String,
    #[serde(default)]
    media_type: Option<String>,
}

fn publish_call(arguments: Value, call: String) -> Result<PublishCall, String> {
    let arguments: PublishArguments = serde_json::from_value(arguments)
        .map_err(|error| format!("artifacts_publish's arguments: {error}"))?;
    Ok(PublishCall {
        environment: arguments.environment,
        path: arguments.path,
        media_type: arguments.media_type,
        call,
    })
}

/// What an `artifacts_publish` came to, as `PublishAnswer` says it on the
/// host: an error unless the conversation holds the file.
fn published(answer: &PublishAnswer) -> Value {
    match answer {
        PublishAnswer::Kept {
            already_held,
            name,
            digest,
            size,
        } => result(
            json!({
                "outcome": if *already_held { "alreadyHeld" } else { "held" },
                "digest": digest,
                "size": size,
                "name": name,
            }),
            false,
        ),
        PublishAnswer::Refused(refusal) => result(
            json!({
                "refused": publish_refusal_code(*refusal),
                "message": publish_refusal_message(*refusal),
            }),
            true,
        ),
    }
}

fn publish_refusal_code(refusal: PublishCallRefusal) -> &'static str {
    match refusal {
        PublishCallRefusal::NoTurn => "no_turn",
        PublishCallRefusal::EnvironmentNotLeased => "environment_not_leased",
        PublishCallRefusal::DuplicateRequest => "duplicate_request",
        PublishCallRefusal::NotAllowed => "not_allowed",
        PublishCallRefusal::PublishUnavailable => "publish_unavailable",
        PublishCallRefusal::Unanswered => "unanswered",
        PublishCallRefusal::TimedOut => "timed_out",
        PublishCallRefusal::Cancelled => "cancelled",
        PublishCallRefusal::NotPublished(reason) => match reason {
            PublishRefusal::OutsideWorkspace => "outside_workspace",
            PublishRefusal::Unreadable => "unreadable",
            PublishRefusal::Size => "size",
            PublishRefusal::Invalid => "invalid",
            PublishRefusal::Busy => "busy",
            PublishRefusal::StagingFailed => "staging_failed",
            PublishRefusal::AuditUnavailable => "audit_unavailable",
            PublishRefusal::LeaseEnded => "lease_ended",
        },
        PublishCallRefusal::NotKept(reason) => match reason {
            CollectionRefusal::BudgetExceeded => "budget_exceeded",
            CollectionRefusal::Invalid => "invalid",
            CollectionRefusal::ChannelUnavailable => "channel_unavailable",
            CollectionRefusal::Mismatch => "mismatch",
            CollectionRefusal::NotKept => "not_kept",
            CollectionRefusal::LeaseEnded => "lease_ended",
        },
    }
}

fn publish_refusal_message(refusal: PublishCallRefusal) -> &'static str {
    match refusal {
        PublishCallRefusal::NoTurn => "no turn of this conversation is running to publish it for",
        PublishCallRefusal::EnvironmentNotLeased => {
            "the agent does not run there; publish from the environment environments_list marks current"
        }
        PublishCallRefusal::DuplicateRequest => "this call is already publishing a file",
        PublishCallRefusal::NotAllowed => {
            "this turn's Drive grant does not allow artifacts_publish; nothing was published"
        }
        PublishCallRefusal::PublishUnavailable => {
            "where the agent runs publishes no files yet"
        }
        PublishCallRefusal::Unanswered => {
            "the environment was lost before it answered; the file may or may not be held"
        }
        PublishCallRefusal::TimedOut => {
            "the file was not kept in time; it may still be held once it is read"
        }
        PublishCallRefusal::Cancelled => "the call was cancelled",
        PublishCallRefusal::NotPublished(reason) => match reason {
            PublishRefusal::OutsideWorkspace => {
                "the path is not a regular file inside the workspace"
            }
            PublishRefusal::Unreadable => "no such file, or it cannot be read",
            PublishRefusal::Size => "the file is empty or larger than a file may be",
            PublishRefusal::Invalid => "the media type or file name cannot be used",
            PublishRefusal::Busy => "as many files as may be in flight are being published",
            PublishRefusal::StagingFailed => "the environment could not copy the file",
            PublishRefusal::AuditUnavailable => "the environment could not record the publish",
            PublishRefusal::LeaseEnded => "the agent's lease ended first",
        },
        PublishCallRefusal::NotKept(reason) => match reason {
            CollectionRefusal::BudgetExceeded => {
                "as many files or bytes as this lease may publish are held"
            }
            CollectionRefusal::Invalid => "what the environment said of the file cannot be kept",
            CollectionRefusal::ChannelUnavailable => "the file could not be read from the environment",
            CollectionRefusal::Mismatch => "the bytes read were not the file published",
            CollectionRefusal::NotKept => "the conversation could not keep the file",
            CollectionRefusal::LeaseEnded => "the agent's lease ended before the file was kept",
        },
    }
}

/// `artifacts_fetch`'s arguments, as its input schema says them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FetchArguments {
    environment: String,
    digest: String,
    media_type: String,
    size: u64,
    destination: String,
}

fn fetch_call(arguments: Value, call: String) -> Result<FetchCall, String> {
    let arguments: FetchArguments = serde_json::from_value(arguments)
        .map_err(|error| format!("artifacts_fetch's arguments: {error}"))?;
    Ok(FetchCall {
        environment: arguments.environment,
        digest: arguments.digest,
        media_type: arguments.media_type,
        size: arguments.size,
        destination: arguments.destination,
        call,
    })
}

/// What an `artifacts_fetch` came to: an error unless the file is where
/// the answer says and its end is recorded here.
fn fetched(answer: &FetchAnswer) -> Value {
    match answer {
        FetchAnswer::Placed {
            already_placed,
            path,
            digest,
            size,
            recorded,
        } => result(
            json!({
                "outcome": if *already_placed { "alreadyPlaced" } else { "placed" },
                "path": path,
                "digest": digest,
                "size": size,
                "recorded": recorded,
            }),
            !*recorded,
        ),
        FetchAnswer::Refused(refusal) | FetchAnswer::RefusedUnrecorded(refusal) => {
            let mut said = json!({
                "refused": fetch_refusal_code(*refusal),
                "message": fetch_refusal_message(*refusal),
            });
            // With no answer, whether every byte was delivered is the one
            // thing that says whether the file may yet be placed.
            if let FetchCallRefusal::Unanswered { delivered }
            | FetchCallRefusal::TimedOut { delivered }
            | FetchCallRefusal::Cancelled { delivered } = refusal
            {
                said["delivered"] = json!(delivered);
            }
            // Its end not saved here: said, as a placement's is.
            if let FetchAnswer::RefusedUnrecorded(_) = answer {
                said["recorded"] = json!(false);
            }
            result(said, true)
        }
    }
}

fn fetch_refusal_code(refusal: FetchCallRefusal) -> &'static str {
    match refusal {
        FetchCallRefusal::NoTurn => "no_turn",
        FetchCallRefusal::EnvironmentNotLeased => "environment_not_leased",
        FetchCallRefusal::DuplicateRequest => "duplicate_request",
        FetchCallRefusal::NotAllowed => "not_allowed",
        FetchCallRefusal::FetchUnavailable => "fetch_unavailable",
        FetchCallRefusal::Invalid => "invalid",
        FetchCallRefusal::NotHeld => "not_held",
        FetchCallRefusal::BudgetExceeded => "budget_exceeded",
        FetchCallRefusal::Busy => "busy",
        FetchCallRefusal::AuditUnavailable => "audit_unavailable",
        FetchCallRefusal::SourceUnavailable => "source_unavailable",
        FetchCallRefusal::ChannelUnavailable => "channel_unavailable",
        FetchCallRefusal::Unanswered { .. } => "unanswered",
        FetchCallRefusal::TimedOut { .. } => "timed_out",
        FetchCallRefusal::Cancelled { .. } => "cancelled",
        FetchCallRefusal::NotPlaced(reason) => match reason {
            FetchRefusal::Unavailable => "fetch_unavailable",
            FetchRefusal::OutsideWorkspace => "outside_workspace",
            FetchRefusal::NoDirectory => "no_directory",
            FetchRefusal::Occupied => "occupied",
            FetchRefusal::Size => "size",
            FetchRefusal::Invalid => "invalid",
            FetchRefusal::Busy => "busy",
            FetchRefusal::Mismatch => "mismatch",
            FetchRefusal::PlacingFailed => "placing_failed",
            FetchRefusal::AuditUnavailable => "audit_unavailable",
            FetchRefusal::LeaseEnded => "lease_ended",
        },
    }
}

fn fetch_refusal_message(refusal: FetchCallRefusal) -> &'static str {
    match refusal {
        FetchCallRefusal::NoTurn => "no turn of this conversation is running to fetch it for",
        FetchCallRefusal::EnvironmentNotLeased => {
            "the agent does not run there; fetch into the environment environments_list marks current"
        }
        FetchCallRefusal::DuplicateRequest => "this call is already fetching a file",
        FetchCallRefusal::NotAllowed => {
            "this turn's Drive grant does not allow artifacts_fetch; nothing was placed"
        }
        FetchCallRefusal::FetchUnavailable => "where the agent runs takes no files",
        FetchCallRefusal::Invalid => {
            "the digest, media type, size or destination cannot name a file"
        }
        FetchCallRefusal::NotHeld => {
            "this conversation does not hold that file for the person asking, or let go of it; nothing was placed"
        }
        FetchCallRefusal::BudgetExceeded => {
            "as many bytes as this lease may fetch were fetched"
        }
        FetchCallRefusal::Busy => "as many files as may be in flight are being fetched",
        FetchCallRefusal::AuditUnavailable => {
            "the fetch could not be recorded, so the environment was not asked; nothing was placed"
        }
        FetchCallRefusal::SourceUnavailable => {
            "the file could not be read here; nothing was placed"
        }
        FetchCallRefusal::ChannelUnavailable => {
            "the file could not be written to the environment; nothing was placed"
        }
        FetchCallRefusal::Unanswered { delivered: true } => {
            "the environment was lost after the file was written; it may or may not be placed"
        }
        FetchCallRefusal::Unanswered { delivered: false } => {
            "no answer came before the file was written; nothing was placed"
        }
        FetchCallRefusal::TimedOut { delivered: true } => {
            "the file was not placed in time; it may still be placed"
        }
        FetchCallRefusal::TimedOut { delivered: false } => {
            "the file was not written in time; nothing was placed"
        }
        FetchCallRefusal::Cancelled { delivered: true } => {
            "the call was cancelled; the file may still be placed"
        }
        FetchCallRefusal::Cancelled { delivered: false } => {
            "the call was cancelled before the file was written; nothing was placed"
        }
        FetchCallRefusal::NotPlaced(reason) => match reason {
            FetchRefusal::Unavailable => "the environment does not take files",
            FetchRefusal::OutsideWorkspace => {
                "the destination is not a place inside the workspace"
            }
            FetchRefusal::NoDirectory => "the destination's directory does not exist",
            FetchRefusal::Occupied => {
                "something other than this file is at the destination; it was not replaced"
            }
            FetchRefusal::Size => "the file is empty or larger than a file may be",
            FetchRefusal::Invalid => "the environment could not read the request",
            FetchRefusal::Busy => "as many files as may be in flight are being fetched",
            FetchRefusal::Mismatch => "the bytes written were not the file asked for",
            FetchRefusal::PlacingFailed => "the environment could not place the file",
            FetchRefusal::AuditUnavailable => "the environment could not record the fetch",
            FetchRefusal::LeaseEnded => "the agent's lease ended first",
        },
    }
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
            // A command that exited holds a process, so `NotHeld` beside it
            // confirms nothing.
            let succeeded = ran.exit == CommandExit::Exited { code: 0 }
                && *recorded
                && matches!(ran.cleanup, Some(LeaseCleanup::Confirmed { .. }));
            result(
                json!({
                    "lease": lease.as_str(),
                    "exit": exit(ran.exit),
                    "stdout": String::from_utf8_lossy(&ran.stdout),
                    "stderr": String::from_utf8_lossy(&ran.stderr),
                    "droppedBytes": ran.dropped_bytes,
                    "cleanup": cleanup(ran),
                    "recorded": recorded,
                    "published": publications(&ran.published),
                }),
                !succeeded,
            )
        }
    }
}

fn publications(published: &[CommandPublication]) -> Vec<Value> {
    published
        .iter()
        .map(|published| {
            let outcome = match published.outcome {
                PublicationOutcome::Held => json!("held"),
                PublicationOutcome::AlreadyHeld => json!("alreadyHeld"),
                PublicationOutcome::Refused(reason) => json!({ "refused": reason }),
            };
            json!({
                "name": published.name,
                "digest": published.digest,
                "size": published.size,
                "outcome": outcome,
            })
        })
        .collect()
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
        LeaseEndCause::Idle => "idle",
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
        CommandRefusal::Environment(LeaseRefusal::EnvironmentPlatformUnsupported) => {
            "environment_platform_unsupported"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentInstallFailed) => {
            "environment_install_failed"
        }
        CommandRefusal::Environment(LeaseRefusal::AgentUnavailable) => "agent_unavailable",
        CommandRefusal::EnvironmentNotGranted
        | CommandRefusal::Environment(LeaseRefusal::EnvironmentNotGranted) => {
            "environment_not_granted"
        }
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
        CommandRefusal::Environment(LeaseRefusal::EnvironmentPlatformUnsupported) => {
            "the host has no copy of Nessa, and runs on a system this build cannot run on"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentInstallFailed) => {
            "the host had no copy of Nessa, and installing one failed"
        }
        CommandRefusal::Environment(LeaseRefusal::AgentUnavailable) => "the host cannot run this",
        CommandRefusal::EnvironmentNotGranted => {
            "commands are not granted on that environment; see environments_list"
        }
        CommandRefusal::Environment(LeaseRefusal::EnvironmentNotGranted) => {
            "the environment's owner has not granted this gateway that command there"
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
        CommandCallError::NotAllowed(reason) => reason,
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
