use super::super::{
    executions::{
        event_queue::{EventQueueBudget, EventReceiver},
        prompt_content::{
            encoded_image_bytes, fits_one_frame, read_images, ImageBlocks, IMAGE_READ_TIMEOUT,
        },
        steering::RESPONSE_TIMEOUT,
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
    PermissionSelectionState,
};
use crate::application::agent_execution::providers::{
    CleanupFuture, CleanupReport, ExecutionEventStream, ExecutionReport, ImageInputRefusal,
    ObservationFailure, ObservationFailureCause, OpenedProviderSession, ProviderCleanup,
    ProviderExecutionFuture, ProviderExecutionReply, ProviderObservationFuture, ProviderOpenError,
    ProviderOperationCapabilities, ProviderOperationFailure, ProviderOperationFuture,
    ProviderOperationResult, ProviderSession, ProviderSessionBackend, ProviderSessionState,
    ResourceCleanup, SessionCloseRequest, SteeringOutcome,
};
use crate::domain::agent_execution::executions::ExecutionId;
use crate::domain::agent_execution::prompts::UserMessage;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::domain::effective_capabilities::value_objects::EffectiveCapabilities;
use crate::infrastructure::process::{ProcessScope, ProcessStartFailure};
use std::{
    sync::{atomic::AtomicU64, Arc, Mutex as ControlMutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch, Mutex, MutexGuard, OwnedSemaphorePermit, Semaphore},
    time::Instant,
};

pub(crate) type ProcessFactory =
    Arc<dyn Fn() -> Result<ProcessScope, ProcessStartFailure> + Send + Sync>;

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
    let (operation_capabilities, _) = watch::channel(ProviderOperationCapabilities::default());
    let session_audit = audit.clone();
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
    let (mut generation, initial_events) = factory.start(restore)?;
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
    let image_budget = Arc::new(Semaphore::new(in_flight_image_bytes(&factory.config)));
    let session = Arc::new(AcpSession {
        id: session_id.clone(),
        factory,
        generation: Mutex::new(generation),
        control,
        close_activity,
        event_generations,
        image_budget,
    });
    Ok(OpenedProviderSession {
        session: ProviderSession::new(session_id, session, capabilities, session_audit),
        events: Box::new(Events {
            current: Some(initial_events),
            queued,
        }),
    })
}

struct WorkerFactory<P> {
    event_budget: EventQueueBudget,
    operation_capabilities: watch::Sender<ProviderOperationCapabilities>,
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
    ) -> Result<(Generation, EventStream), ProviderOpenError> {
        self.operation_capabilities
            .send_replace(ProviderOperationCapabilities::default());
        let scope = match (self.process)() {
            Ok(scope) => scope,
            Err(failure) => {
                let (cause, recovery) = failure.into_parts();
                return Err(match recovery {
                    Some(directory) => ProviderOpenError::with_cleanup(
                        cause,
                        Arc::new(ProcessCleanup::retaining_directory(
                            self.config.clone(),
                            directory,
                        )),
                    ),
                    None => ProviderOpenError::no_resources(cause),
                });
            }
        };
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
                restoration: None,
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
    restoration: Option<Arc<RestorationRecovery>>,
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
enum Control {
    Generation {
        recovery: Arc<ProcessCleanup>,
        close_requested: watch::Sender<Option<SessionCloseRequest>>,
        completion: watch::Receiver<Option<Completion>>,
    },
    Restoration(Arc<RestorationRecovery>),
}
struct RestorationRecovery {
    cause: AgentError,
    cleanup: Arc<dyn ProviderCleanup>,
}
struct LiveGenerationFailure {
    cause: AgentError,
    state: Option<Box<ProviderSessionState>>,
}
impl Generation {
    fn control(&self) -> Control {
        Control::Generation {
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
impl RestorationRecovery {
    async fn retry(&self) -> CleanupReport {
        self.cleanup
            .retry_cleanup()
            .await
            .with_operation_failure(Some(self.cause.clone()))
    }

    fn failure(&self, state: ProviderSessionState) -> LiveGenerationFailure {
        LiveGenerationFailure {
            cause: self.cause.clone(),
            state: Some(Box::new(state)),
        }
    }

    fn operation_failure(&self, state: ProviderSessionState) -> ProviderOperationFailure {
        ProviderOperationFailure::new(self.cause.clone(), state)
    }
}
impl From<AgentError> for LiveGenerationFailure {
    fn from(cause: AgentError) -> Self {
        Self { cause, state: None }
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

/// Encoded image bytes one session may hold outside the worker at once.
///
/// Every queued command retains its own images, so without this the sixteen
/// slots of the command queue would each be allowed a whole frame of them.
/// Two frames is what the worker can ever be asked for at once, one execution
/// and one native steering, and a message that passes admission always fits
/// one frame, so a single message can never be refused for want of budget.
fn in_flight_image_bytes(config: &AcpConfig) -> usize {
    config.max_frame_bytes.saturating_mul(2)
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
    // Retained by the blocks of every message read but not yet written.
    image_budget: Arc<Semaphore>,
}
impl<P: AcpProfile + Clone> AcpSession<P> {
    async fn live_generation(&self) -> Result<MutexGuard<'_, Generation>, LiveGenerationFailure> {
        let close_activity = self.close_activity.subscribe();
        let mut generation = self.generation.lock().await;
        if close_activity.has_changed().unwrap_or(true) {
            return Err(AgentError::Closed.into());
        }
        if generation.startup.is_some() && !generation.stopped() {
            let restored = generation.ready().await?;
            if restored != self.id {
                generation.pending_events.take();
                return Err(AgentError::Protocol("restored a different session".into()).into());
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
                    return Err(error.into());
                }
            }
            if let Some(recovery) = generation.restoration.clone() {
                let report = recovery.retry().await;
                if !report.is_confirmed() {
                    return Err(recovery.failure(ProviderSessionState::CleanupReported(report)));
                }
                generation.restoration = None;
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
                    return Err(AgentError::Closed.into());
                }
                let (next, events) = match self.factory.start(Some(self.id.clone())) {
                    Ok(started) => started,
                    Err(failure) => {
                        let (cause, cleanup) = failure.into_parts();
                        if let Some(cleanup) = cleanup {
                            let recovery = Arc::new(RestorationRecovery {
                                cause: cause.clone(),
                                cleanup,
                            });
                            let failure = recovery.failure(ProviderSessionState::CleanupRequired);
                            generation.restoration = Some(recovery.clone());
                            *control = Control::Restoration(recovery);
                            return Err(failure);
                        }
                        return Err(cause.into());
                    }
                };
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
                return Err(AgentError::Protocol("restored a different session".into()).into());
            }
            generation.publish_events();
        }
        if close_activity.has_changed().unwrap_or(true) || generation.stopped() {
            return Err(AgentError::Closed.into());
        }
        Ok(generation)
    }
}
impl<P: AcpProfile + Clone + Sync> ProviderSessionBackend for AcpSession<P> {
    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        *self.factory.operation_capabilities.borrow()
    }
    fn validate_input(&self, input: &ExecutionRequest) -> Result<(), AgentError> {
        self.factory
            .profile
            .validate_execution(input, &self.factory.capabilities)?;
        fits_one_frame(&input.user_message, self.factory.config.max_frame_bytes)
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
            // Subscribed before the context is made live, so a close at any
            // later moment is seen by the image read below.
            let closing = self.close_activity.subscribe();
            let (commands, observations) = {
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
                (generation.commands.clone(), generation.observations.clone())
            };
            // The generation lock is released: a stalled source must not hold
            // up steering, permission answers, or the next restoration. The
            // execution's own deadline starts here, before the read, so reading
            // spends it instead of adding to it.
            let execution_timeout = self.factory.config.execution_timeout;
            let deadline = execution_timeout.map(|limit| Instant::now() + limit);
            let limit =
                execution_timeout.map_or(IMAGE_READ_TIMEOUT, |limit| limit.min(IMAGE_READ_TIMEOUT));
            let images = match self
                .images(&input.user_message, &commands, closing, limit)
                .await
            {
                Ok(images) => images,
                Err(error) => return ProviderExecutionReply::Rejected(error),
            };
            let prompt = DispatchedPrompt {
                input,
                images,
                deadline,
            };
            // A generation that stopped meanwhile has closed this queue, so the
            // request is refused rather than handed to a different process.
            if let Err(error) = enqueue(&commands, Command::ExecutionRequest(prompt, sender)) {
                return ProviderExecutionReply::Rejected(error);
            }
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
            let closing = self.close_activity.subscribe();
            let commands = {
                let generation = self.generation.lock().await;
                if generation.stopped() {
                    return Err(ProviderOperationFailure::new(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                    ));
                }
                generation.commands.clone()
            };
            // Read with the lock released and off the worker's task: the
            // active execution keeps being driven, and can finish, meanwhile.
            // The steering deadline starts here, before the read, so the whole
            // call is answered within it rather than within it twice over.
            let deadline = Instant::now() + RESPONSE_TIMEOUT;
            let limit = IMAGE_READ_TIMEOUT.min(RESPONSE_TIMEOUT);
            let images = self
                .images(&input.user_message, &commands, closing, limit)
                .await
                .map_err(|error| {
                    let state = if error == AgentError::Closed {
                        ProviderSessionState::CleanupRequired
                    } else {
                        ProviderSessionState::Usable
                    };
                    ProviderOperationFailure::new(error, state)
                })?;
            let prompt = DispatchedPrompt {
                input,
                images,
                deadline: Some(deadline),
            };
            enqueue(&commands, Command::Steer(target, prompt, sender)).map_err(|error| {
                ProviderOperationFailure::new(error, ProviderSessionState::Usable)
            })?;
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
                    return Err(ProviderOperationFailure::permission_answer(
                        AgentError::Closed,
                        ProviderSessionState::CleanupRequired,
                        PermissionSelectionState::Pending,
                    ));
                }
                enqueue(&generation.commands, Command::Answer(answer, sender)).map_err(
                    |error| {
                        ProviderOperationFailure::permission_answer(
                            error,
                            ProviderSessionState::Usable,
                            PermissionSelectionState::Pending,
                        )
                    },
                )?;
            }
            receiver.await.unwrap_or_else(|_| {
                Err(ProviderOperationFailure::permission_answer(
                    AgentError::Closed,
                    ProviderSessionState::CleanupRequired,
                    PermissionSelectionState::Unknown,
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
            enum CloseTarget {
                Generation(watch::Receiver<Option<Completion>>, Arc<ProcessCleanup>),
                Restoration(Arc<RestorationRecovery>),
            }
            let target = {
                let control = self.control.lock().expect("session control lock");
                self.close_activity.send_replace(());
                match &*control {
                    Control::Generation {
                        recovery,
                        close_requested,
                        completion,
                    } => {
                        close_requested.send_if_modified(|current| {
                            if current.is_none() {
                                *current = Some(request.clone());
                                true
                            } else {
                                false
                            }
                        });
                        CloseTarget::Generation(completion.clone(), recovery.clone())
                    }
                    Control::Restoration(recovery) => CloseTarget::Restoration(recovery.clone()),
                }
            };
            let (receiver, recovery) = match target {
                CloseTarget::Generation(receiver, recovery) => (receiver, recovery),
                CloseTarget::Restoration(recovery) => return recovery.retry().await,
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
    /// The image blocks for `message`, read before its command is queued so
    /// the worker never waits on the injected source.
    ///
    /// The caller holds no lock. The read is abandoned, with
    /// [`AgentError::Closed`], when this context is closed or the generation
    /// behind `commands` stops for any reason (consumer loss and a failed
    /// process included), and it fails after `limit`. What could refuse the
    /// images outright is answered first, so nothing is read for a message
    /// that was never going to be sent; the worker repeats those checks when
    /// it dispatches.
    ///
    /// The session's share of in-flight encoded bytes is taken before the first
    /// read and travels with the blocks. A message that cannot have it is
    /// [`AgentError::Busy`], again with nothing read.
    async fn images(
        &self,
        message: &UserMessage,
        commands: &mpsc::Sender<Command>,
        mut closing: watch::Receiver<()>,
        limit: Duration,
    ) -> Result<ImageBlocks, AgentError> {
        if !message.images().is_empty() {
            if self.factory.config.images.is_none() {
                return Err(AgentError::ImageInputRefused(ImageInputRefusal::NotOffered));
            }
            let agent = self.operation_capabilities();
            if agent.negotiated && !agent.image_input {
                return Err(AgentError::ImageInputRefused(
                    ImageInputRefusal::AgentDoesNotAccept,
                ));
            }
        }
        let charge = self.charge_images(message)?;
        let stopped = async {
            tokio::select! {
                _ = closing.changed() => {}
                () = commands.closed() => {}
            }
        };
        let blocks = read_images(
            self.factory.config.images.as_deref(),
            message,
            limit,
            stopped,
        )
        .await?;
        Ok(blocks.charged(charge))
    }
    /// This message's share of the bytes one session may hold outside the
    /// worker, taken before anything is read so a refusal costs no read.
    fn charge_images(
        &self,
        message: &UserMessage,
    ) -> Result<Option<OwnedSemaphorePermit>, AgentError> {
        let bytes = encoded_image_bytes(message);
        if bytes == 0 {
            return Ok(None);
        }
        self.image_budget
            .clone()
            .try_acquire_many_owned(bytes)
            .map(Some)
            .map_err(|_| AgentError::Busy)
    }
    async fn operation_failure(&self, failure: LiveGenerationFailure) -> ProviderOperationFailure {
        if let Some(state) = failure.state {
            return ProviderOperationFailure::new(failure.cause, *state);
        }
        let error = failure.cause;
        let generation = self.generation.lock().await;
        if let Some(recovery) = generation.restoration.as_ref() {
            return recovery.operation_failure(ProviderSessionState::CleanupRequired);
        }
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

/// One user message on its way to the worker: already admitted, with its
/// images read and encoded, and with what is left of the deadline of the phase
/// that began before that read.
pub(crate) struct DispatchedPrompt {
    pub input: ExecutionRequest,
    pub images: ImageBlocks,
    /// When this phase must be finished. `None` only for an execution the host
    /// left unbounded in time; the frame's own write allowance may extend it.
    pub deadline: Option<Instant>,
}

pub(crate) enum Command {
    /// Native steering for the identified execution.
    Steer(
        ExecutionId,
        DispatchedPrompt,
        oneshot::Sender<ProviderOperationResult<SteeringOutcome>>,
    ),
    /// One prompt.
    ExecutionRequest(DispatchedPrompt, oneshot::Sender<ProviderExecutionReply>),
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

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/sessions/restoration_recovery.rs"]
mod restoration_recovery_tests;
