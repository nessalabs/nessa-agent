use super::{
    binding::{ClaudeAcpConfig, Command, Completion},
    process::ProcessScope,
    wire::{self, Envelope, RpcId},
};
use crate::{
    application::agent_binding::*,
    domain::effective_capabilities::value_objects::EffectiveCapabilities,
};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::ChildStdout,
    sync::{mpsc, oneshot, watch},
    time::{timeout, Instant},
};

type PromptReply = oneshot::Sender<Result<PromptOutcome, BindingError>>;
struct ActivePrompt {
    id: i64,
    execution_id: String,
    reply: PromptReply,
}
struct Permission {
    wire_id: RpcId,
    allow: String,
    reject: String,
}
struct Reader {
    stdout: ChildStdout,
    buffer: Vec<u8>,
    limit: usize,
}
impl Reader {
    async fn next(&mut self) -> Result<Envelope, BindingError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
                if end > self.limit {
                    return Err(wire::protocol("frame exceeds configured limit"));
                }
                let frame: Vec<_> = self.buffer.drain(..=end).collect();
                if frame.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return wire::parse(&frame);
            }
            if self.buffer.len() > self.limit {
                return Err(wire::protocol("frame exceeds configured limit"));
            }
            let mut bytes = [0; 8192];
            let count = self
                .stdout
                .read(&mut bytes)
                .await
                .map_err(|_| BindingError::Transport("stdout read failed".into()))?;
            if count == 0 {
                return Err(BindingError::Transport("provider stdout closed".into()));
            }
            self.buffer.extend_from_slice(&bytes[..count]);
        }
    }
}
struct Worker {
    scope: ProcessScope,
    reader: Reader,
    config: ClaudeAcpConfig,
    capabilities: EffectiveCapabilities,
    commands: mpsc::Receiver<Command>,
    stop: watch::Receiver<bool>,
    events: mpsc::Sender<BindingEvent>,
    session: String,
    sequence: i64,
    permission_sequence: u64,
    active: Option<ActivePrompt>,
    permissions: HashMap<String, Permission>,
    tool_names: HashMap<String, String>,
    deadline: Option<Instant>,
    stopping: bool,
    deferred_outcome: Option<PromptOutcome>,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    mut scope: ProcessScope,
    config: ClaudeAcpConfig,
    capabilities: EffectiveCapabilities,
    commands: mpsc::Receiver<Command>,
    stop: watch::Receiver<bool>,
    finished: watch::Sender<Option<Completion>>,
    events: mpsc::Sender<BindingEvent>,
    ready: oneshot::Sender<Result<(), BindingError>>,
) {
    let reader = Reader {
        stdout: scope.stdout.take().expect("owned stdout"),
        buffer: Vec::new(),
        limit: config.max_frame_bytes,
    };
    let deadline = Some(Instant::now() + config.startup_timeout);
    let mut worker = Worker {
        scope,
        reader,
        config,
        capabilities,
        commands,
        stop,
        events,
        session: String::new(),
        sequence: 0,
        permission_sequence: 0,
        active: None,
        permissions: HashMap::new(),
        tool_names: HashMap::new(),
        deadline,
        stopping: false,
        deferred_outcome: None,
    };
    let startup = worker.startup().await;
    let mut ready = Some(ready);
    let result = match startup {
        Ok(()) => {
            if ready.take().expect("startup sender").send(Ok(())).is_err() {
                Err(BindingError::Closed)
            } else {
                worker.drive().await
            }
        }
        Err(error) => Err(error),
    };
    // One teardown path for startup, protocol errors, Stop, timeout and handle drop.
    let _ = timeout(worker.config.shutdown_grace, worker.cancel_permissions()).await;
    if !worker.session.is_empty() {
        let _ = worker
            .send(wire::notification(
                "session/cancel",
                json!({"sessionId":worker.session}),
            ))
            .await;
    }
    let cleanup = worker
        .scope
        .cleanup(worker.config.shutdown_grace, worker.config.kill_timeout)
        .await;
    let mut failure = cleanup
        .as_ref()
        .err()
        .cloned()
        .or_else(|| result.as_ref().err().cloned());
    if worker.active.is_some() {
        let outcome = if cleanup.is_err() {
            Err(BindingError::CleanupUncertain)
        } else if let Some(outcome) = worker.deferred_outcome {
            Ok(outcome)
        } else if worker.stopping && result.is_ok() {
            Ok(PromptOutcome::Cancelled)
        } else {
            Err(result.clone().err().unwrap_or(BindingError::Closed))
        };
        if let Err(error) = worker.emit(BindingUpdate::Finished(outcome.clone())) {
            if failure.is_none() {
                failure = Some(error);
            }
        }
        let active = worker.active.take().expect("active prompt");
        let _ = active.reply.send(outcome);
    }
    if let Some(ready) = ready {
        let _ = ready.send(Err(if cleanup.is_err() {
            BindingError::CleanupUncertain
        } else {
            result.err().unwrap_or(BindingError::Closed)
        }));
    }
    // Cleanup evidence is separate from the prompt's known execution outcome.
    let _ = finished.send(Some(Completion { cleanup, failure }));
}
impl Worker {
    fn next_id(&mut self) -> Result<i64, BindingError> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| wire::protocol("request ID exhausted"))?;
        Ok(self.sequence)
    }
    async fn send(&mut self, value: Value) -> Result<(), BindingError> {
        let mut bytes =
            serde_json::to_vec(&value).map_err(|_| wire::protocol("could not encode request"))?;
        if bytes.len() > self.config.max_frame_bytes {
            return Err(BindingError::InvalidInput(
                "encoded request exceeds frame limit".into(),
            ));
        }
        bytes.push(b'\n');
        let stdin = self.scope.stdin.as_mut().ok_or(BindingError::Closed)?;
        timeout(Duration::from_secs(1), stdin.write_all(&bytes))
            .await
            .map_err(|_| BindingError::Deadline)?
            .map_err(|_| BindingError::Transport("stdin write failed".into()))
    }
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, BindingError> {
        let id = self.next_id()?;
        self.send(wire::request(id, method, params)).await?;
        loop {
            if *self.stop.borrow() {
                return Err(BindingError::Closed);
            }
            let message = tokio::select! { biased;
                _ = self.stop.changed() => return Err(BindingError::Closed),
                _ = wait_for_deadline(self.deadline) => return Err(BindingError::Deadline),
                message = self.reader.next() => message?,
            };
            if let Some(method) = message.method {
                if let Some(id) = message.id {
                    let response = if method == "session/request_permission" {
                        wire::permission_cancel(&id)
                    } else {
                        wire::unsupported(&id)
                    };
                    self.send(response).await?;
                }
                continue;
            }
            if message.id != Some(RpcId::Number(id)) {
                return Err(wire::protocol("unexpected startup response"));
            }
            if let Some(error) = message.error {
                return Err(BindingError::Provider { code: error.code });
            }
            return message
                .result
                .ok_or_else(|| wire::protocol("missing response result"));
        }
    }
    async fn startup(&mut self) -> Result<(), BindingError> {
        let init = self.rpc("initialize", json!({"protocolVersion":1,"clientInfo":{"name":"nessa-sdk","version":env!("CARGO_PKG_VERSION")},
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}})).await?;
        if init.get("protocolVersion").and_then(Value::as_u64) != Some(1)
            || init.pointer("/agentInfo/version").and_then(Value::as_str) != Some("0.76.0")
        {
            return Err(wire::protocol("requires Claude ACP 0.76.0 and protocol 1"));
        }
        let tools = if self.config.file_tools {
            wire::FILE_TOOLS
        } else {
            &[]
        };
        let result = self.rpc("session/new", json!({"cwd":self.config.workspace,"mcpServers":[],"_meta":{"claudeCode":{"options":{
            "model":self.capabilities.model().model_id(),"settingSources":[],"tools":tools,"permissionMode":"default",
            "settings":{"disableAllHooks":true,"allowedMcpServers":[],"disableClaudeAiConnectors":true,
                "permissions":{"defaultMode":"default","ask":tools}}
        }}}})).await?;
        self.session = wire::identifier(&result, "sessionId")?.to_owned();
        wire::verify_config(&result, self.capabilities.model().model_id(), false)?;
        let result = self
            .rpc(
                "session/set_config_option",
                json!({"sessionId":self.session,"configId":"mode","value":"default"}),
            )
            .await?;
        wire::verify_config(&result, self.capabilities.model().model_id(), true)
    }
    async fn drive(&mut self) -> Result<(), BindingError> {
        enum Input {
            Stop,
            Command(Option<Command>),
            Message(Result<Envelope, BindingError>),
            Deadline,
            ConsumerGone,
        }
        loop {
            let input = if *self.stop.borrow() && !self.stopping {
                Input::Stop
            } else {
                tokio::select! { biased;
                    _ = self.stop.changed(), if !self.stopping => Input::Stop,
                    _ = self.events.closed() => Input::ConsumerGone,
                    _ = wait_for_deadline(self.deadline), if self.active.is_some() || self.stopping => Input::Deadline,
                    command = self.commands.recv(), if !self.stopping => Input::Command(command),
                    message = self.reader.next() => Input::Message(message),
                }
            };
            match input {
                Input::Stop | Input::Command(None) => {
                    self.stopping = true;
                    timeout(self.config.shutdown_grace, self.cancel_permissions())
                        .await
                        .map_err(|_| BindingError::Deadline)??;
                    self.send(wire::notification(
                        "session/cancel",
                        json!({"sessionId":self.session}),
                    ))
                    .await?;
                    self.deadline = Some(Instant::now() + self.config.shutdown_grace);
                    if self.active.is_none() {
                        return Ok(());
                    }
                }
                Input::ConsumerGone => return Err(BindingError::Backpressure),
                Input::Deadline => {
                    return if self.stopping {
                        Ok(())
                    } else {
                        Err(BindingError::Deadline)
                    }
                }
                Input::Command(Some(command)) => self.command(command).await?,
                Input::Message(message) => {
                    self.message(message?).await?;
                    if self.deferred_outcome.is_some() || (self.stopping && self.active.is_none()) {
                        return Ok(());
                    }
                }
            }
        }
    }
    async fn command(&mut self, command: Command) -> Result<(), BindingError> {
        match command {
            Command::Prompt(input, reply) => {
                if reply.is_closed() {
                    return Ok(());
                }
                if self.active.is_some() {
                    let _ = reply.send(Err(BindingError::Busy));
                    return Ok(());
                }
                let validation = validate_prompt(&input, &self.capabilities);
                let validation = validation.and_then(|()| {
                    if input.text.trim().is_empty() || input.reserved_output_tokens != self.capabilities.limits().max_output() {
                        Err(BindingError::InvalidInput("output reservation must equal this binding's configured output ceiling".into()))
                    } else { Ok(()) }
                });
                if let Err(error) = validation {
                    let _ = reply.send(Err(error));
                    return Ok(());
                }
                let id = self.next_id()?;
                self.active = Some(ActivePrompt {
                    id,
                    execution_id: input.execution_id.clone(),
                    reply,
                });
                self.tool_names.clear();
                self.deadline = self
                    .config
                    .prompt_timeout
                    .map(|limit| Instant::now() + limit);
                self.send(wire::request(
                    id,
                    "session/prompt",
                    json!({"sessionId":self.session,"prompt":[{"type":"text","text":input.text}]}),
                ))
                .await?;
            }
            Command::Answer(answer, reply) => {
                if reply.is_closed() {
                    return Ok(());
                }
                if *self.stop.borrow() {
                    let _ = reply.send(Err(BindingError::Closed));
                    return Ok(());
                }
                if self
                    .active
                    .as_ref()
                    .is_none_or(|active| active.execution_id != answer.execution_id)
                {
                    let _ = reply.send(Err(BindingError::StalePermission));
                    return Ok(());
                }
                if let Some(permission) = self.permissions.remove(&answer.id) {
                    let response = wire::selected(
                        &permission.wire_id,
                        if answer.allow_once {
                            &permission.allow
                        } else {
                            &permission.reject
                        },
                    );
                    let result = self.send(response).await;
                    let _ = reply.send(result.clone());
                    result?;
                } else {
                    let _ = reply.send(Err(BindingError::StalePermission));
                }
            }
        }
        Ok(())
    }
    async fn message(&mut self, message: Envelope) -> Result<(), BindingError> {
        if let Some(method) = message.method {
            let params = message.params.unwrap_or(Value::Null);
            if let Some(id) = message.id {
                if method == "session/request_permission" {
                    let result = self.permission(id.clone(), params).await;
                    if result.is_err()
                        && !self
                            .permissions
                            .values()
                            .any(|permission| permission.wire_id == id)
                    {
                        let _ = self.send(wire::permission_cancel(&id)).await;
                    }
                    result?;
                } else {
                    self.send(wire::unsupported(&id)).await?;
                }
            } else if method == "session/update" {
                self.update(params)?;
            }
            return Ok(());
        }
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| wire::protocol("response without active prompt"))?;
        if message.id != Some(RpcId::Number(active.id)) {
            return Err(wire::protocol("response for a different prompt"));
        }
        if let Some(error) = message.error {
            let error = BindingError::Provider { code: error.code };
            let _ = self.emit(BindingUpdate::Finished(Err(error.clone())));
            let active = self.active.take().expect("validated active prompt");
            let _ = active.reply.send(Err(error.clone()));
            return Err(error);
        }
        let result = wire::outcome(
            &message
                .result
                .ok_or_else(|| wire::protocol("missing prompt result"))?,
        )?;
        if result == PromptOutcome::Cancelled {
            // Never publish cancellation from protocol evidence alone.
            self.deferred_outcome = Some(result);
        } else {
            let published = self.emit(BindingUpdate::Finished(Ok(result)));
            let active = self.active.take().expect("validated active prompt");
            let _ = active.reply.send(Ok(result));
            published?;
        }
        timeout(self.config.shutdown_grace, self.cancel_permissions())
            .await
            .map_err(|_| BindingError::Deadline)??;
        Ok(())
    }
    fn emit(&self, update: BindingUpdate) -> Result<(), BindingError> {
        let active = self
            .active
            .as_ref()
            .ok_or_else(|| wire::protocol("event has no execution"))?;
        self.events
            .try_send(BindingEvent {
                execution_id: active.execution_id.clone(),
                update,
            })
            .map_err(|_| BindingError::Backpressure)
    }
    fn check_session(&self, params: &Value) -> Result<(), BindingError> {
        if wire::identifier(params, "sessionId")? != self.session {
            return Err(wire::protocol("message belongs to another session"));
        }
        Ok(())
    }
    fn update(&mut self, params: Value) -> Result<(), BindingError> {
        self.check_session(&params)?;
        let update = params
            .get("update")
            .ok_or_else(|| wire::protocol("missing session update"))?;
        let kind = wire::string(update, "sessionUpdate")?;
        match kind {
            "config_option_update" => {
                return wire::verify_config(update, self.capabilities.model().model_id(), true)
            }
            "current_mode_update" => {
                if wire::string(update, "currentModeId")? != "default" {
                    return Err(wire::protocol("permission mode changed"));
                }
                return Ok(());
            }
            "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update" => {
                if self.active.is_none() {
                    return Err(wire::protocol("execution update without an active prompt"));
                }
            }
            // Bounded advisory updates cannot change the immutable capability snapshot.
            _ => return Ok(()),
        }
        if kind == "agent_message_chunk" || kind == "agent_thought_chunk" {
            let content = update
                .get("content")
                .ok_or_else(|| wire::protocol("missing message content"))?;
            if content.get("type").and_then(Value::as_str) != Some("text") {
                return Err(wire::protocol("non-text output is not supported"));
            }
            let text = content
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| wire::protocol("invalid text content"))?
                .to_owned();
            self.emit(if kind == "agent_message_chunk" {
                BindingUpdate::Text(text)
            } else {
                BindingUpdate::Thought(text)
            })
        } else {
            if !self.config.file_tools {
                return Err(wire::protocol("tool event in a text-only binding"));
            }
            let tool = wire::tool_call(update, &mut self.tool_names)?;
            self.emit(BindingUpdate::Tool(tool))
        }
    }
    async fn permission(&mut self, id: RpcId, params: Value) -> Result<(), BindingError> {
        self.check_session(&params)?;
        if self.stopping || self.active.is_none() || !self.config.file_tools {
            return self.send(wire::permission_cancel(&id)).await;
        }
        if self.permissions.len() >= 128 || self.permissions.values().any(|p| p.wire_id == id) {
            return Err(wire::protocol("permission request limit or duplicate ID"));
        }
        let tool = params
            .get("toolCall")
            .ok_or_else(|| wire::protocol("missing permission tool"))?;
        let tool_id = wire::identifier(tool, "toolCallId")?;
        let name = self
            .tool_names
            .get(tool_id)
            .ok_or_else(|| wire::protocol("permission has no observed file tool"))?
            .clone();
        let input = wire::file_input(
            &name,
            tool.get("rawInput")
                .ok_or_else(|| wire::protocol("missing tool input"))?,
        )?;
        let tool = wire::tool_call(tool, &mut self.tool_names)?;
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .ok_or_else(|| wire::protocol("missing permission options"))?;
        let mut option_ids = std::collections::HashSet::new();
        for option in options {
            if !option_ids.insert(wire::identifier(option, "optionId")?) {
                return Err(wire::protocol("duplicate permission option ID"));
            }
        }
        let option = |kind| -> Result<String, BindingError> {
            let matches: Vec<_> = options
                .iter()
                .filter(|option| option.get("kind").and_then(Value::as_str) == Some(kind))
                .collect();
            if matches.len() != 1 {
                return Err(wire::protocol("requires one once-only permission option"));
            }
            Ok(wire::identifier(matches[0], "optionId")?.to_owned())
        };
        let permission = Permission {
            wire_id: id,
            allow: option("allow_once")?,
            reject: option("reject_once")?,
        };
        self.permission_sequence = self
            .permission_sequence
            .checked_add(1)
            .ok_or_else(|| wire::protocol("permission ID exhausted"))?;
        let id = self.permission_sequence.to_string();
        self.permissions.insert(id.clone(), permission);
        self.emit(BindingUpdate::PermissionRequested {
            id,
            tool,
            input: Box::new(input),
        })
    }
    async fn cancel_permissions(&mut self) -> Result<(), BindingError> {
        let permissions = std::mem::take(&mut self.permissions);
        for permission in permissions.into_values() {
            self.send(wire::permission_cancel(&permission.wire_id))
                .await?;
        }
        Ok(())
    }
}

/// No artificial far-future timestamp: no configured limit means no timer.
async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
