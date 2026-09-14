use super::super::{
    executions::{
        event_queue::{EventQueueBudget, EventReceiver},
        worker,
    },
    profile::AcpProfile,
};
use super::{cleanup::ProcessCleanup, AcpConfig};
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::executions::{
    ExecutionAudit, ExecutionEvent, ExecutionRequest,
};

use crate::application::agent_execution::permissions::{
    PermissionAnswer, PermissionCancellation, PermissionCancellationRequest, PermissionResolution,
};
use crate::application::agent_execution::providers::{
    CleanupFuture, CleanupReport, ExecutionEventStream, ExecutionReport, ObservationFailure,
    ObservationFailureCause, OpenedProviderSession, OperationCapabilities, ProviderCleanup,
    ProviderExecutionFuture, ProviderExecutionReply, ProviderObservationFuture, ProviderOpenError,
    ProviderOperationFailure, ProviderOperationFuture, ProviderOperationResult, ProviderSession,
    ProviderSessionBackend, ProviderSessionState, ResourceCleanup, SessionCloseRequest,
    SteeringOutcome,
};
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::process::ProcessScope;
use std::sync::{atomic::AtomicU64, Arc, Mutex as ControlMutex};
use tokio::sync::{mpsc, oneshot, watch, Mutex, MutexGuard};

pub(crate) type ProcessFactory = Arc<dyn Fn() -> Result<ProcessScope, AgentError> + Send + Sync>;

pub(crate) async fn open<P: AcpProfile + Clone + Sync>(
    process: ProcessFactory,
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    profile: P,
    audit: Arc<dyn ExecutionAudit>,
    restore: Option<ExecutionSessionId>,
) -> Result<OpenedProviderSession, ProviderOpenError> {
    config.validate().map_err(|cause| {
        // Validation runs before allocation and returns only Configuration/Unsupported.
        ProviderOpenError::no_resources(cause)
    })?;
    let (operation_capabilities, _) = watch::channel(OperationCapabilities::default());
    let factory = WorkerFactory {
        event_budget: EventQueueBudget::new(),
        operation_capabilities,
        process,
        config,
        capabilities: capabilities.clone(),
        profile,
        audit,
        permission_sequence: Arc::new(AtomicU64::new(0)),
    };
    let (mut generation, initial_events) = factory.start(restore).map_err(|cause| {
        // This private factory returns only ProcessScope::spawn Transport errors;
        // it transfers a scope only on success, before the worker is started.
        ProviderOpenError::no_resources(cause)
    })?;
    // A cancelled open drops this generation and requests its process cleanup.
    let session_id = match generation.ready().await {
        Ok(id) => id,
        Err(cause) => {
            let completed = completion(generation.completion.clone()).await;
            return Err(match completed {
                Ok(completed) if completed.cleanup.is_confirmed() => {
                    ProviderOpenError::no_resources(cause)
                }
                _ => ProviderOpenError::with_cleanup(cause, generation.recovery.clone()),
            });
        }
    };
    let (event_generations, queued) = mpsc::channel(16);
    let control = ControlMutex::new(generation.control());
    let (close_activity, _) = watch::channel(());
    let session = Arc::new(AcpSession {
        id: session_id.clone(),
        factory,
        generation: Mutex::new(generation),
        control,
        close_activity,
        event_generations,
    });
    Ok(OpenedProviderSession {
        session: ProviderSession::new(session_id, session, capabilities),
        events: Box::new(Events {
            current: Some(initial_events),
            queued,
        }),
    })
}

struct WorkerFactory<P> {
    event_budget: EventQueueBudget,
    operation_capabilities: watch::Sender<OperationCapabilities>,
    process: ProcessFactory,
    config: AcpConfig,
    capabilities: EffectiveCapabilities,
    profile: P,
    audit: Arc<dyn ExecutionAudit>,
    permission_sequence: Arc<AtomicU64>,
}
impl<P: AcpProfile + Clone> WorkerFactory<P> {
    fn start(
        &self,
        restore: Option<ExecutionSessionId>,
    ) -> Result<(Generation, EventStream), AgentError> {
        self.operation_capabilities
            .send_replace(OperationCapabilities::default());
        let scope = (self.process)()?;
        let (commands, receiver) = mpsc::channel(16);
        let (close_requested, close_receiver) = watch::channel(None);
        let (finished, completion) = watch::channel(None);
        let (events, event_receiver) = self.event_budget.channel(self.config.event_capacity);
        let (ready, startup) = oneshot::channel();
        let observations = Arc::new(ControlMutex::new(GenerationObservations::default()));
        let recovery = Arc::new(ProcessCleanup::new(self.config.clone()));
        tokio::spawn(worker::run(
            scope,
            self.profile.clone(),
            self.config.clone(),
            self.capabilities.clone(),
            receiver,
            close_receiver,
            finished,
            events,
            ready,
            self.audit.clone(),
            restore,
            self.permission_sequence.clone(),
            self.operation_capabilities.clone(),
            recovery.clone(),
        ));
        Ok((
            Generation {
                recovery,
                commands,
                close_requested,
                completion: completion.clone(),
                startup: Some(startup),
                pending_events: None,
                observations: observations.clone(),
            },
            EventStream {
                receiver: event_receiver,
                completion,
                exhausted: false,
                observations,
            },
        ))
    }
}

#[derive(Default, PartialEq, Eq)]
enum FailureDelivery {
    #[default]
    Unreported,
    Reported,
    Retired,
}
#[derive(Default)]
struct GenerationObservations {
    // A retired reader still drains its observations; its failure must not be
    // attached to a new invocation after already reaching an operation or reader.
    delivery: FailureDelivery,
    // Execution acknowledgement can precede Completion on another thread.
    // Compare the returned error once completion becomes available.
    operation_failure: Option<AgentError>,
}

struct Generation {
    recovery: Arc<ProcessCleanup>,
    commands: mpsc::Sender<Command>,
    close_requested: watch::Sender<Option<SessionCloseRequest>>,
    completion: watch::Receiver<Option<Completion>>,
    startup: Option<oneshot::Receiver<Result<ExecutionSessionId, AgentError>>>,
    // A cancelled preparation keeps both the worker and its reserved reader slot.
    // Only a ready generation joins the public stream; failed startup has no
    // invocation observations, and its failure/cleanup travels in the operation report.
    pending_events: Option<(mpsc::OwnedPermit<EventStream>, EventStream)>,
    observations: Arc<ControlMutex<GenerationObservations>>,
}
struct Control {
    recovery: Arc<ProcessCleanup>,
    close_requested: watch::Sender<Option<SessionCloseRequest>>,
    completion: watch::Receiver<Option<Completion>>,
}
impl Generation {
    fn control(&self) -> Control {
        Control {
            recovery: self.recovery.clone(),
            close_requested: self.close_requested.clone(),
            completion: self.completion.clone(),
        }
    }
    async fn ready(&mut self) -> Result<ExecutionSessionId, AgentError> {
        let result = self
            .startup
            .as_mut()
            .expect("generation startup pending")
            .await;
        self.startup = None;
        let result = result.unwrap_or(Err(AgentError::Closed));
        if result.is_err() {
            self.pending_events.take();
            self.observations
                .lock()
                .expect("generation observation lock")
                .delivery = FailureDelivery::Reported;
        }
        result
    }
    fn publish_events(&mut self) {
        if let Some((permit, events)) = self.pending_events.take() {
            permit.send(events);
        }
    }
    fn request_close(&self, request: SessionCloseRequest) {
        self.close_requested.send_if_modified(|current| {
            if current.is_none() {
                *current = Some(request.clone());
                true
            } else {
                false
            }
        });
    }
    fn stopped(&self) -> bool {
        self.close_requested.borrow().is_some()
            || self.completion.borrow().is_some()
            || self.commands.is_closed()
    }
}
impl Drop for Generation {
    fn drop(&mut self) {
        self.request_close(SessionCloseRequest::SessionHandlesDropped);
    }
}
async fn completion(
    mut receiver: watch::Receiver<Option<Completion>>,
) -> Result<Completion, AgentError> {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return Ok(result);
        }
        receiver
            .changed()
            .await
            .map_err(|_| AgentError::CleanupUncertain)?;
    }
}
fn enqueue(sender: &mpsc::Sender<Command>, command: Command) -> Result<(), AgentError> {
    sender.try_send(command).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => AgentError::Busy,
        mpsc::error::TrySendError::Closed(_) => AgentError::Closed,
    })
}

struct AcpSession<P> {
    id: ExecutionSessionId,
    factory: WorkerFactory<P>,
    generation: Mutex<Generation>,
    // Only synchronous signal/installation operations hold this lock. Close never
    // waits for the generation lock that serializes asynchronous restoration.
    control: ControlMutex<Control>,
    close_activity: watch::Sender<()>,
    event_generations: mpsc::Sender<EventStream>,
}
impl<P: AcpProfile + Clone> AcpSession<P> {
    async fn live_generation(&self) -> Result<MutexGuard<'_, Generation>, AgentError> {
        let close_activity = self.close_activity.subscribe();
        let mut generation = self.generation.lock().await;
        if close_activity.has_changed().unwrap_or(true) {
            return Err(AgentError::Closed);
        }
        if generation.startup.is_some() && !generation.stopped() {
            let restored = generation.ready().await?;
            if restored != self.id {
                generation.pending_events.take();
                return Err(AgentError::Protocol("restored a different session".into()));
            }
            generation.publish_events();
        }
        if generation.stopped() {
            generation.pending_events.take();
            let completed = completion(generation.completion.clone()).await?;
            let cleanup = match generation.recovery.confirmed().await {
                Some(outcome) => completed
                    .cleanup
                    .clone()
                    .with_resources(ResourceCleanup::Confirmed(outcome)),
                None => completed.cleanup.clone(),
            };
            cleanup.into_result()?;
            if let Some(error) = completed.failure {
                let mut observations = generation
                    .observations
                    .lock()
                    .expect("generation observation lock");
                let reported = observations.delivery != FailureDelivery::Unreported
                    || observations.operation_failure.as_ref() == Some(&error);
                observations.delivery = FailureDelivery::Reported;
                if !reported {
                    // Preparation must surface an idle failure before a restored
                    // worker can accept the caller's next prompt.
                    return Err(error);
                }
            }
            // Reserve the reader before starting another process. A dropped consumer
            // or an undrained sequence of generations cannot create hidden workers.
            let permit =
                self.event_generations.clone().try_reserve_owned().map_err(
                    |error| match error {
                        mpsc::error::TrySendError::Full(_) => AgentError::Backpressure,
                        mpsc::error::TrySendError::Closed(_) => AgentError::Backpressure,
                    },
                )?;
            {
                let mut control = self.control.lock().expect("session control lock");
                if close_activity.has_changed().unwrap_or(true) {
                    return Err(AgentError::Closed);
                }
                let (next, events) = self.factory.start(Some(self.id.clone()))?;
                generation
                    .observations
                    .lock()
                    .expect("generation observation lock")
                    .delivery = FailureDelivery::Retired;
                *control = next.control();
                *generation = next;
                generation.pending_events = Some((permit, events));
            }
            // Store the generation before awaiting startup: cancelling execute leaves
            // its worker tracked, so the next attempt waits for that same startup.
            let restored = generation.ready().await?;
            if restored != self.id {
                generation.pending_events.take();
                return Err(AgentError::Protocol("restored a different session".into()));
            }
            generation.publish_events();
        }
        if close_activity.has_changed().unwrap_or(true) || generation.stopped() {
            return Err(AgentError::Closed);
        }
        Ok(generation)
    }
}
impl<P: AcpProfile + Clone + Sync> ProviderSessionBackend for AcpSession<P> {
    fn operation_capabilities(&self) -> OperationCapabilities {
        *self.factory.operation_capabilities.borrow()
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async move {
            match self.live_generation().await {
                Ok(generation) => {
                    drop(generation);
                    Ok(())
                }
                Err(error) => Err(self.operation_failure(error).await),
            }
        })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel();
            let observations = {
                let generation = match self.live_generation().await {
                    Ok(generation) => generation,
                    Err(error) => {
                        let failure = self.operation_failure(error).await;
                        let (error, attachment) = failure.into_parts();
                        return ProviderExecutionReply::Finished(ExecutionReport::new(
                            None,
                            Some(error),
                            attachment,
                        ));
                    }
                };
                if let Err(error) = enqueue(
                    &generation.commands,
                    Command::ExecutionRequest(input, sender),
                ) {
                    return ProviderExecutionReply::Rejected(error);
                }
                generation.observations.clone()
            };
            let result = receiver.await.unwrap_or_else(|_| {
                ProviderExecutionReply::Finished(ExecutionReport::new(
                    None,
                    Some(AgentError::Closed),
                    ProviderSessionState::CleanupRequired,
                ))
            });
            if let Err(error) = result.clone().into_result() {
                observations
                    .lock()
                    .expect("generation observation lock")
                    .operation_failure = Some(error);
            }
            result
        })
    }
    fn steer(
        &self,
        target: ExecutionId,
        input: ExecutionRequest,
    ) -> ProviderOperationFuture<'_, SteeringOutcome> {
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel();
            {
                let generation = self.generation.lock().await;
                if generation.stopped() {
                    return Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    ));
                }
                enqueue(&generation.commands, Command::Steer(target, input, sender)).map_err(
                    |error| ProviderOperationFailure::new(error, ProviderSessionState::Usable),
                )?;
            }
            receiver.await.unwrap_or_else(|_| {
                Err(ProviderOperationFailure::new(
                    AgentError::Closed,
                    ProviderSessionState::CleanupRequired,
                ))
            })
        })
    }
    fn answer_permission(
        &self,
        answer: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel();
            {
                let generation = self.generation.lock().await;
                if generation.stopped() {
                    return Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    ));
                }
                enqueue(&generation.commands, Command::Answer(answer, sender)).map_err(
                    |error| ProviderOperationFailure::new(error, ProviderSessionState::Usable),
                )?;
            }
            receiver.await.unwrap_or_else(|_| {
                Err(ProviderOperationFailure::new(
                    AgentError::Closed,
                    ProviderSessionState::CleanupRequired,
                ))
            })
        })
    }
    fn cancel_permission(
        &self,
        input: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel();
            {
                let generation = self.generation.lock().await;
                if generation.stopped() {
                    return Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    ));
                }
                enqueue(
                    &generation.commands,
                    Command::CancelPermission(input, sender),
                )
                .map_err(|error| {
                    ProviderOperationFailure::new(error, ProviderSessionState::Usable)
                })?;
            }
            receiver.await.unwrap_or_else(|_| {
                Err(ProviderOperationFailure::new(
                    AgentError::Closed,
                    ProviderSessionState::CleanupRequired,
                ))
            })
        })
    }
    fn close(&self, request: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async move {
            let (receiver, recovery) = {
                let control = self.control.lock().expect("session control lock");
                self.close_activity.send_replace(());
                control.close_requested.send_if_modified(|current| {
                    if current.is_none() {
                        *current = Some(request.clone());
                        true
                    } else {
                        false
                    }
                });
                (control.completion.clone(), control.recovery.clone())
            };
            let completed = match completion(receiver).await {
                Ok(completed) => completed,
                Err(error) => return CleanupReport::unconfirmed(error),
            };
            if completed.cleanup.is_confirmed() {
                return completed.cleanup;
            }
            let retry = recovery.retry_cleanup().await;
            completed.cleanup.with_resources(retry.resources().clone())
        })
    }
}
impl<P: AcpProfile + Clone> AcpSession<P> {
    async fn operation_failure(&self, error: AgentError) -> ProviderOperationFailure {
        let generation = self.generation.lock().await;
        let completed = generation.completion.borrow().clone();
        let disposition = match completed {
            Some(completed) => {
                ProviderSessionState::CleanupReported(match generation.recovery.confirmed().await {
                    Some(outcome) => completed
                        .cleanup
                        .with_resources(ResourceCleanup::Confirmed(outcome)),
                    None => completed.cleanup,
                })
            }
            None if generation.stopped() => ProviderSessionState::CleanupRequired,
            None => ProviderSessionState::Usable,
        };
        ProviderOperationFailure::new(error, disposition)
    }
}

pub(crate) enum Command {
    Steer(
        ExecutionId,
        ExecutionRequest,
        oneshot::Sender<ProviderOperationResult<SteeringOutcome>>,
    ),
    ExecutionRequest(ExecutionRequest, oneshot::Sender<ProviderExecutionReply>),
    CancelPermission(
        PermissionCancellationRequest,
        oneshot::Sender<ProviderOperationResult<PermissionCancellation>>,
    ),
    Answer(
        PermissionAnswer,
        oneshot::Sender<ProviderOperationResult<PermissionResolution>>,
    ),
}
#[derive(Clone)]
pub(crate) struct Completion {
    pub cleanup: CleanupReport,
    pub failure: Option<AgentError>,
    pub failure_cause: ObservationFailureCause,
}

struct EventStream {
    receiver: EventReceiver,
    completion: watch::Receiver<Option<Completion>>,
    exhausted: bool,
    observations: Arc<ControlMutex<GenerationObservations>>,
}
impl EventStream {
    async fn next(&mut self) -> Result<Option<ExecutionEvent>, ObservationFailure> {
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
                failure_cause,
                ..
            }) => {
                let mut observations = self
                    .observations
                    .lock()
                    .expect("generation observation lock");
                if observations.delivery == FailureDelivery::Retired {
                    Ok(None)
                } else {
                    observations.delivery = FailureDelivery::Reported;
                    Err(ObservationFailure::new(error.clone(), *failure_cause))
                }
            }
            Some(_) => Ok(None),
            None => Err(ObservationFailure::new(
                AgentError::CleanupUncertain,
                ObservationFailureCause::ExecutionFailed,
            )),
        }
    }
}
/// One public reader drains each process generation in order. A previously
/// exhausted reader becomes usable again after execute restores the session.
struct Events {
    current: Option<EventStream>,
    queued: mpsc::Receiver<EventStream>,
}
impl ExecutionEventStream for Events {
    fn next(&mut self) -> ProviderObservationFuture<'_> {
        Box::pin(async move {
            loop {
                if let Some(current) = &mut self.current {
                    if let Some(event) = current.next().await? {
                        return Ok(Some(event));
                    }
                }
                match self.queued.try_recv() {
                    Ok(next) => self.current = Some(next),
                    Err(_) => return Ok(None),
                }
            }
        })
    }
}
