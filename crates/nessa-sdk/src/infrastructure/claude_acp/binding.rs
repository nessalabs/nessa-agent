use super::{process::ProcessScope, worker};
use crate::domain::agent_execution::events::*;
use crate::domain::agent_execution::value_objects::*;
use crate::{
    application::agent_binding::*,
    domain::{
        common::value_objects::TokenLimits,
        effective_capabilities::value_objects::{BindingRestrictions, EffectiveCapabilities},
        model_metadata::{
            entities::ModelMetadata,
            value_objects::{Modalities, ModelFeatures, ModelProvider},
        },
    },
};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, watch};

/// Host-owned launch configuration. Environment is explicit, never inherited by
/// the adapter. Use the normal HOME/auth environment without extracting secrets.
/// Only trusted composition may choose the executable and its arguments.
#[derive(Clone)]
pub struct ClaudeAcpConfig {
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: BTreeMap<OsString, OsString>,
    pub workspace: PathBuf,
    pub file_tools: bool,
    pub permissions: PermissionConfig,
    pub startup_timeout: Duration,
    /// None leaves execution unbounded in time (the default policy).
    /// Some sets an explicit total runtime limit, not a stuck-agent detector.
    pub prompt_timeout: Option<Duration>,
    pub shutdown_grace: Duration,
    pub kill_timeout: Duration,
    pub event_capacity: usize,
    pub max_frame_bytes: usize,
}
/// Immutable composition factory; opening twice creates independent process scopes.
pub struct ClaudeAcpBinding {
    config: ClaudeAcpConfig,
    capabilities: EffectiveCapabilities,
}
impl ClaudeAcpBinding {
    pub fn new(
        config: ClaudeAcpConfig,
        model: &ModelMetadata,
        limits: TokenLimits,
    ) -> Result<Self, BindingError> {
        if !cfg!(unix) {
            return Err(BindingError::Unsupported("native Claude process supervision requires Unix; Windows needs an owned Job Object adapter".into()));
        }
        if config.permissions.allows(PermissionDecision::AllowAlways)
            || config.permissions.allows(PermissionDecision::RejectAlways)
        {
            return Err(BindingError::Unsupported("persistent Claude permissions require modeled durable rule scopes; this adapter currently supports once-only choices".into()));
        }
        if model.key().provider() != ModelProvider::Anthropic {
            return Err(BindingError::Configuration(
                "Claude requires an Anthropic model".into(),
            ));
        }
        if !config.executable.is_absolute() || !config.workspace.is_absolute() {
            return Err(BindingError::Configuration(
                "executable and workspace must be absolute paths".into(),
            ));
        }
        if [
            config.startup_timeout,
            config.shutdown_grace,
            config.kill_timeout,
        ]
        .iter()
        .chain(config.prompt_timeout.iter())
        .any(|duration| {
            duration.is_zero() || tokio::time::Instant::now().checked_add(*duration).is_none()
        }) || !(1..=4096).contains(&config.event_capacity)
            || !(1024..=16 * 1024 * 1024).contains(&config.max_frame_bytes)
        {
            return Err(BindingError::Configuration(
                "positive deadlines and bounded frame/event capacities are required".into(),
            ));
        }
        let text = Modalities::new(true, false, false).expect("text modality is nonempty");
        let restrictions = BindingRestrictions::new(
            ModelFeatures::new(text, text, config.file_tools, false),
            // This first profile deliberately excludes extended context and
            // larger output modes. These are binding ceilings, not model facts.
            TokenLimits::new(200_000, 64_000).expect("valid native profile ceilings"),
        );
        let capabilities = EffectiveCapabilities::new(model, restrictions, limits)
            .map_err(|e| BindingError::Configuration(e.to_string()))?;
        if config.file_tools && !capabilities.features().tool_use() {
            return Err(BindingError::Configuration(
                "selected model does not support tools".into(),
            ));
        }
        Ok(Self {
            config,
            capabilities,
        })
    }
}
impl AgentBinding for ClaudeAcpBinding {
    fn open(&self) -> BindingFuture<'_, OpenedBinding> {
        Box::pin(async move {
            let scope = ProcessScope::spawn(&self.config, &self.capabilities)?;
            let (commands, receiver) = mpsc::channel(16);
            let (stop, stop_receiver) = watch::channel(false);
            let (finished, completion) = watch::channel(None);
            let (events, event_receiver) = mpsc::channel(self.config.event_capacity);
            let (ready, startup) = oneshot::channel();
            tokio::spawn(worker::run(
                scope,
                self.config.clone(),
                self.capabilities.clone(),
                receiver,
                stop_receiver,
                finished,
                events,
                ready,
            ));
            let session = Arc::new(Session {
                commands,
                stop,
                completion,
            });
            // On caller cancellation, Session drops and signals the owned worker.
            startup.await.map_err(|_| BindingError::Closed)??;
            Ok(OpenedBinding {
                session: session.clone(),
                events: Box::new(Events {
                    receiver: event_receiver,
                    completion: session.completion.clone(),
                    exhausted: false,
                }),
                capabilities: self.capabilities.clone(),
            })
        })
    }
}

pub(super) enum Command {
    PromptRequest(
        PromptRequest,
        oneshot::Sender<Result<PromptOutcome, BindingError>>,
    ),
    Answer(PermissionAnswer, oneshot::Sender<Result<(), BindingError>>),
}
#[derive(Clone)]
pub(super) struct Completion {
    pub cleanup: Result<StopOutcome, BindingError>,
    pub failure: Option<BindingError>,
}
struct Session {
    commands: mpsc::Sender<Command>,
    stop: watch::Sender<bool>,
    completion: watch::Receiver<Option<Completion>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
impl AgentSession for Session {
    fn prompt(&self, input: PromptRequest) -> BindingFuture<'_, PromptOutcome> {
        Box::pin(async move {
            if *self.stop.borrow() {
                return Err(BindingError::Closed);
            }
            let (sender, receiver) = oneshot::channel();
            self.commands
                .try_send(Command::PromptRequest(input, sender))
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => BindingError::Busy,
                    mpsc::error::TrySendError::Closed(_) => BindingError::Closed,
                })?;
            receiver.await.map_err(|_| BindingError::Closed)?
        })
    }
    fn answer_permission(&self, answer: PermissionAnswer) -> BindingFuture<'_, ()> {
        Box::pin(async move {
            if *self.stop.borrow() {
                return Err(BindingError::Closed);
            }
            let (sender, receiver) = oneshot::channel();
            self.commands
                .try_send(Command::Answer(answer, sender))
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => BindingError::Busy,
                    mpsc::error::TrySendError::Closed(_) => BindingError::Closed,
                })?;
            receiver.await.map_err(|_| BindingError::Closed)?
        })
    }
    fn stop(&self) -> BindingFuture<'_, StopOutcome> {
        Box::pin(async move {
            let mut completion = self.completion.clone();
            let _ = self.stop.send(true);
            loop {
                if let Some(result) = completion.borrow().clone() {
                    return result.cleanup;
                }
                completion
                    .changed()
                    .await
                    .map_err(|_| BindingError::CleanupUncertain)?;
            }
        })
    }
}
struct Events {
    receiver: mpsc::Receiver<AgentTurnEvent>,
    completion: watch::Receiver<Option<Completion>>,
    exhausted: bool,
}
impl AgentTurnEvents for Events {
    fn next(&mut self) -> BindingFuture<'_, Option<AgentTurnEvent>> {
        Box::pin(async move {
            if self.exhausted {
                return Ok(None);
            }
            if let Some(event) = self.receiver.recv().await {
                return Ok(Some(event));
            }
            self.exhausted = true;
            match self.completion.borrow().as_ref() {
                Some(Completion {
                    failure: Some(error),
                    ..
                }) => Err(error.clone()),
                Some(_) => Ok(None),
                None => Err(BindingError::CleanupUncertain),
            }
        })
    }
}
