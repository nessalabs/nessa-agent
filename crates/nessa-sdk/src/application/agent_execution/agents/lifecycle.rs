//! Owns this Agent’s live session, accepted work, and provider cleanup.
//! Saved conversation history belongs to SessionManager.
//! SDK tasks keep their work permits until recording and response delivery finish.
//! Cleanup alone does not allow a new execution while accepted work is finishing.
use super::{
    AgentError, AttachmentAuthorization, AttachmentFailure, AttachmentFailureCode, AttachmentPhase,
    AttachmentRequest, AttachmentStatus, AttachmentWait,
};
use crate::application::agent_execution::{
    executions::{
        AttachmentAuditCause, AttachmentAuditRecord, AttachmentAuditStage, ExecutionAudit,
        ExecutionAuditRecord,
    },
    permissions::ActionContext,
    providers::{
        CleanupReport, OperationCapabilities, ProviderOperationFailure, ProviderSessionState,
        SessionCloseRequest,
    },
    sessions::{
        attachment::AttachmentLease, AttachedProvider, InvocationCancellationEvent,
        SessionStorageLease,
    },
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, SchedulingCause},
    sessions::{AttachmentCause, SessionId},
};
#[cfg(test)]
use std::sync::mpsc::Receiver;
use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
#[cfg(test)]
use tokio::sync::oneshot;
use tokio::sync::{watch, Mutex as AsyncMutex};

// Stopping invalidates commands accepted in the previous work generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WorkGeneration(u64);
// Restoring the provider connection starts a new provider generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProviderGeneration(u64);
// Supervisor diagnostics retain identity without keeping completed work admitted.
#[derive(Clone, Copy)]
pub(super) struct ControlOrigin {
    work_generation: WorkGeneration,
    provider_generation: ProviderGeneration,
}

#[derive(Clone)]
pub(super) struct CloseAttempt {
    id: u64,
    pub(super) request: SessionCloseRequest,
    result: watch::Receiver<Option<CleanupReport>>,
}
impl CloseAttempt {
    pub(super) async fn wait(mut self) -> CleanupReport {
        loop {
            if let Some(result) = self.result.borrow().clone() {
                return result;
            }
            if self.result.changed().await.is_err() {
                return CleanupReport::unconfirmed(AgentError::CleanupUncertain);
            }
        }
    }
}
struct Stop {
    ticket: CloseAttempt,
    finalized: bool,
    recovery: RecoveryPolicy,
}
#[derive(Clone, Copy)]
enum RecoveryPolicy {
    Automatic,
    CleanupNotStarted,
    NeedsExplicitClose,
    ExplicitClose,
}
enum WorkStatus {
    Open,
    Stopping(Stop),
    Blocked(Stop),
}
struct Cleanup {
    provider_generation: ProviderGeneration,
    completion: watch::Sender<Option<CleanupReport>>,
    result: watch::Receiver<Option<CleanupReport>>,
}
#[derive(Clone, Copy)]
enum WorkPhase {
    Waiting,
    // Automatic attachment failure owns final settlement for this queued input.
    Settling,
    Active,
    // The execute future was entered; provider dispatch may still be rejected.
    Executing,
}
struct OwnedWork {
    work_generation: WorkGeneration,
    phase: WorkPhase,
    // The first stop affecting this owner; later cleanup/close cannot replace its cause.
    cancellation: watch::Sender<Option<InvocationCancellationEvent>>,
}
struct State {
    work_generation: WorkGeneration,
    provider_generation: ProviderGeneration,
    // Only successful preparation enables controls for this provider generation.
    provider_ready: bool,
    work_status: WorkStatus,
    cleanup: Option<Cleanup>,
    active: Option<(WorkGeneration, ExecutionId)>,
    next_work: u64,
    work: HashMap<u64, OwnedWork>,
    attachment_generation: u64,
    next_attachment_authorization: u64,
    attachment: AttachmentState,
    attachment_evidence_failure: Option<AttachmentFailure>,
    attachment_evidence: Option<watch::Receiver<Option<Result<(), AgentError>>>>,
    automatic_recovery_ready: bool,
}
struct AttachmentAuthority {
    id: u64,
    work_generation: WorkGeneration,
    attachment_generation: u64,
    cause: AttachmentCause,
    actor: Option<ActionContext>,
    cancelled: watch::Sender<bool>,
}
enum AttachmentState {
    Absent {
        recorded: bool,
    },
    Authorized(AttachmentAuthority),
    Abandoning {
        generation: u64,
        recorded: bool,
    },
    Starting {
        generation: u64,
        cause: AttachmentCause,
        recorded: bool,
        result: watch::Sender<Option<Result<(), AgentError>>>,
    },
    Attached {
        generation: u64,
        cause: AttachmentCause,
        provider: AttachedProvider,
    },
    Failed {
        failure: AttachmentFailure,
        recorded: bool,
    },
}
pub(super) struct SessionLifecycle {
    #[cfg(test)]
    work_admission_pause: Mutex<Option<(oneshot::Sender<()>, Receiver<()>)>>,
    #[cfg(test)]
    preparation_pause: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
    #[cfg(test)]
    cleanup_started: Mutex<Option<oneshot::Sender<()>>>,
    state: Mutex<State>,
    resource_transition: AsyncMutex<()>,
    close: watch::Sender<Option<ActionContext>>,
    stop: watch::Sender<()>,
    changed: watch::Sender<()>,
    attachment: Arc<AttachmentLease>,
    _lease: Arc<dyn SessionStorageLease>,
    session_id: SessionId,
    audit: Arc<dyn ExecutionAudit>,
}
pub(super) struct WorkPermit {
    owner: Arc<SessionLifecycle>,
    id: u64,
    binding: Mutex<(WorkGeneration, ProviderGeneration)>,
    stop: watch::Receiver<()>,
    cancellation: watch::Receiver<Option<InvocationCancellationEvent>>,
}
pub(super) struct AttachmentStart {
    pub(super) generation: u64,
    pub(super) cause: AttachmentCause,
    pub(super) actor: Option<ActionContext>,
    pub(super) result: watch::Sender<Option<Result<(), AgentError>>>,
}
impl WorkPermit {
    fn provider_generation_value(&self) -> ProviderGeneration {
        self.binding.lock().expect("work binding").1
    }
    fn generations(&self) -> (WorkGeneration, ProviderGeneration) {
        *self.binding.lock().expect("work binding")
    }
    pub(super) fn activate(&self) -> Result<(), AgentError> {
        self.owner.activate_waiting(self)
    }
    pub(super) fn mark_execution_started(&self) {
        let mut state = self.owner.state.lock().expect("session lifecycle");
        state
            .work
            .get_mut(&self.id)
            .expect("owned work permit")
            .phase = WorkPhase::Executing;
    }
    pub(super) fn execution_started(&self) -> bool {
        let state = self.owner.state.lock().expect("session lifecycle");
        matches!(
            state.work.get(&self.id).expect("owned work permit").phase,
            WorkPhase::Executing
        )
    }
    #[cfg(test)]
    pub(super) fn work_generation(&self) -> WorkGeneration {
        self.binding.lock().expect("work binding").0
    }
    pub(super) fn control_origin(&self) -> ControlOrigin {
        let (work_generation, provider_generation) = self.generations();
        ControlOrigin {
            work_generation,
            provider_generation,
        }
    }
    pub(super) fn cancellation(&self) -> Option<InvocationCancellationEvent> {
        self.cancellation.borrow().clone()
    }
    pub(super) fn stop_notice(&self) -> watch::Receiver<Option<InvocationCancellationEvent>> {
        self.cancellation.clone()
    }
}
impl Drop for WorkPermit {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().expect("session lifecycle");
        state.work.remove(&self.id);
        self.owner.maybe_reopen(&mut state);
        self.owner.changed.send_replace(());
    }
}
impl SessionLifecycle {
    pub(super) fn new(
        attachment: Arc<AttachmentLease>,
        lease: Arc<dyn SessionStorageLease>,
        recorded_context: bool,
        session_id: SessionId,
        audit: Arc<dyn ExecutionAudit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            #[cfg(test)]
            work_admission_pause: Mutex::new(None),
            #[cfg(test)]
            preparation_pause: Mutex::new(None),
            #[cfg(test)]
            cleanup_started: Mutex::new(None),
            resource_transition: AsyncMutex::new(()),
            state: Mutex::new(State {
                work_generation: WorkGeneration(0),
                provider_generation: ProviderGeneration(0),
                provider_ready: false,
                work_status: WorkStatus::Open,
                cleanup: None,
                active: None,
                next_work: 0,
                work: HashMap::new(),
                attachment_generation: 0,
                next_attachment_authorization: 0,
                attachment: AttachmentState::Absent {
                    recorded: recorded_context,
                },
                attachment_evidence_failure: None,
                attachment_evidence: None,
                automatic_recovery_ready: false,
            }),
            close: watch::channel(None).0,
            stop: watch::channel(()).0,
            changed: watch::channel(()).0,
            attachment,
            _lease: lease,
            session_id,
            audit,
        })
    }
    pub(super) fn is_closed(&self) -> bool {
        !matches!(
            self.state.lock().expect("session lifecycle").work_status,
            WorkStatus::Open
        )
    }
    pub(super) fn close_notice(&self) -> watch::Receiver<Option<ActionContext>> {
        self.close.subscribe()
    }
    pub(super) fn attachment_status(&self) -> AttachmentStatus {
        let state = self.state.lock().expect("session lifecycle");
        let phase = Self::phase(&state);
        let failure = match &state.attachment {
            AttachmentState::Failed { failure, .. } => Some(failure.clone()),
            _ => None,
        };
        AttachmentStatus::new(phase, failure, state.attachment_evidence_failure.clone())
    }
    fn phase(state: &State) -> AttachmentPhase {
        match &state.attachment {
            AttachmentState::Absent { .. } => AttachmentPhase::Absent,
            AttachmentState::Authorized(_) | AttachmentState::Abandoning { .. } => {
                AttachmentPhase::Waiting
            }
            AttachmentState::Starting { .. } => AttachmentPhase::Starting,
            AttachmentState::Attached { .. } if state.provider_ready => AttachmentPhase::Attached,
            AttachmentState::Attached { .. } => AttachmentPhase::Starting,
            AttachmentState::Failed { failure, .. } => AttachmentPhase::Failed(failure.code()),
        }
    }
    pub(super) fn operation_capabilities(&self) -> OperationCapabilities {
        let state = self.state.lock().expect("session lifecycle");
        match &state.attachment {
            AttachmentState::Attached { provider, .. } if state.provider_ready => {
                provider.session.operation_capabilities()
            }
            _ => OperationCapabilities::default(),
        }
    }
    pub(super) fn authorize_attachment(
        self: &Arc<Self>,
        request: AttachmentRequest,
    ) -> Result<AttachmentAuthorization, AgentError> {
        let mut state = self.state.lock().expect("session lifecycle");
        if matches!(state.work_status, WorkStatus::Open) {
            if let Some(report) = state
                .cleanup
                .as_ref()
                .and_then(|cleanup| cleanup.result.borrow().clone())
                .filter(|report| report.is_confirmed() && report.audit().is_ok())
            {
                self.attachment.confirm_cleanup(&report);
                state.cleanup = None;
            }
        }
        if !matches!(state.work_status, WorkStatus::Open) || state.cleanup.is_some() {
            return Err(AgentError::Closed);
        }
        let recorded = match &state.attachment {
            AttachmentState::Absent { recorded } => *recorded,
            AttachmentState::Failed { recorded, .. } if !self.attachment.needs_cleanup() => {
                *recorded
            }
            AttachmentState::Authorized(_)
            | AttachmentState::Abandoning { .. }
            | AttachmentState::Starting { .. }
            | AttachmentState::Attached { .. }
            | AttachmentState::Failed { .. } => {
                return Err(AgentError::AttachmentAuthorizationStale)
            }
        };
        let (cause, actor) = match request {
            AttachmentRequest::CallerRequested(actor) => (
                if recorded {
                    AttachmentCause::Reopen
                } else {
                    AttachmentCause::Initial
                },
                Some(actor),
            ),
            AttachmentRequest::AutomaticRecovery if recorded && state.automatic_recovery_ready => {
                (AttachmentCause::AutomaticRecovery, None)
            }
            AttachmentRequest::AutomaticRecovery => {
                return Err(AgentError::AttachmentAuthorizationStale)
            }
        };
        let id = state.next_attachment_authorization;
        state.next_attachment_authorization += 1;
        let (cancelled, cancellation) = watch::channel(false);
        let authority = AttachmentAuthority {
            id,
            work_generation: state.work_generation,
            attachment_generation: state.attachment_generation,
            cause,
            actor: actor.clone(),
            cancelled,
        };
        state.attachment = AttachmentState::Authorized(authority);
        Ok(AttachmentAuthorization {
            id,
            work_generation: state.work_generation.0,
            attachment_generation: state.attachment_generation,
            cause,
            actor,
            owner: Arc::downgrade(self),
            cancelled: cancellation,
            consumed: false,
        })
    }
    pub(super) fn start_attachment(
        &self,
        mut authorization: AttachmentAuthorization,
    ) -> Result<(AttachmentStart, AttachmentWait), AgentError> {
        let mut state = self.state.lock().expect("session lifecycle");
        let matches = matches!(&state.attachment, AttachmentState::Authorized(authority)
            if authority.id == authorization.id
                && authority.work_generation.0 == authorization.work_generation
                && authority.attachment_generation == authorization.attachment_generation
                && authority.cause == authorization.cause
                && authority.actor == authorization.actor)
            && state.work_generation.0 == authorization.work_generation
            && state.attachment_generation == authorization.attachment_generation
            && matches!(state.work_status, WorkStatus::Open)
            && state.cleanup.is_none();
        if !matches {
            return Err(AgentError::AttachmentAuthorizationStale);
        }
        authorization.consumed = true;
        let (result, wait) = watch::channel(None);
        let start = AttachmentStart {
            generation: state.attachment_generation,
            cause: authorization.cause,
            actor: authorization.actor.clone(),
            result: result.clone(),
        };
        // Starting a replacement establishes its resource generation before
        // provider I/O begins, so a delayed control from the retired attachment
        // cannot report cleanup against the replacement while it is opening.
        state.provider_generation.0 += 1;
        state.attachment = AttachmentState::Starting {
            generation: start.generation,
            cause: start.cause,
            recorded: !matches!(start.cause, AttachmentCause::Initial),
            result,
        };
        Ok((start, AttachmentWait { result: wait }))
    }
    pub(super) fn abandon_attachment_authorization(
        self: &Arc<Self>,
        authorization: &AttachmentAuthorization,
    ) {
        let (generation, recorded, cause, record, completion) = {
            let mut state = self.state.lock().expect("session lifecycle");
            let matching = matches!(&state.attachment, AttachmentState::Authorized(authority)
                if authority.id == authorization.id
                    && authority.work_generation.0 == authorization.work_generation
                    && authority.attachment_generation == authorization.attachment_generation);
            if !matching {
                return;
            }
            if let AttachmentState::Authorized(authority) = &state.attachment {
                authority.cancelled.send_replace(true);
            }
            let recorded = !matches!(authorization.cause, AttachmentCause::Initial);
            let cause = authorization.cause;
            state.attachment_generation += 1;
            let generation = state.attachment_generation;
            state.attachment = AttachmentState::Abandoning {
                generation,
                recorded,
            };
            let record = AttachmentAuditRecord::new(
                self.session_id.clone(),
                AttachmentAuditStage::Waiting,
                AttachmentAuditStage::Absent,
                AttachmentAuditCause::AuthorizationAbandoned,
                authorization.actor.clone(),
            );
            let (completion, result) = watch::channel(None);
            state.attachment_evidence = Some(result);
            self.changed.send_replace(());
            (generation, recorded, cause, record, completion)
        };
        let owner = self.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            let mut state = self.state.lock().expect("session lifecycle");
            let failure = AttachmentFailure::new(
                AttachmentFailureCode::Audit,
                generation,
                cause,
                AgentError::Protocol("attachment abandonment has no runtime".into()),
            );
            state.attachment_evidence_failure = Some(failure.clone());
            if state.attachment_generation == generation
                && matches!(state.attachment, AttachmentState::Abandoning { generation: current, .. } if current == generation)
            {
                state.attachment = AttachmentState::Failed { failure, recorded };
            }
            completion.send_replace(Some(Err(AgentError::AuditFailure)));
            self.changed.send_replace(());
            return;
        };
        runtime.spawn(async move {
            let outcome = owner
                .record_attachment_audit(ExecutionAuditRecord::Attachment(record))
                .await;
            let mut state = owner.state.lock().expect("session lifecycle");
            if state.attachment_generation == generation
                && matches!(state.attachment, AttachmentState::Abandoning { generation: current, .. } if current == generation)
            {
                if let Err(error) = &outcome {
                    let failure = AttachmentFailure::new(
                        AttachmentFailureCode::Audit,
                        generation,
                        cause,
                        error.clone(),
                    );
                    state.attachment_evidence_failure = Some(failure.clone());
                    state.attachment = AttachmentState::Failed {
                        failure,
                        recorded,
                    };
                } else {
                    state.attachment = AttachmentState::Absent { recorded };
                }
                owner.changed.send_replace(());
            } else if let Err(error) = &outcome {
                state.attachment_evidence_failure = Some(AttachmentFailure::new(
                    AttachmentFailureCode::Audit,
                    generation,
                    cause,
                    error.clone(),
                ));
                owner.changed.send_replace(());
            }
            completion.send_replace(Some(outcome));
        });
    }
    pub(super) async fn record_attachment_audit(
        &self,
        record: ExecutionAuditRecord,
    ) -> Result<(), AgentError> {
        let future = catch_unwind(AssertUnwindSafe(|| self.audit.record(record)));
        let Ok(mut future) = future else {
            return Err(AgentError::AuditFailure);
        };
        let outcome = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
                Ok(poll) => poll.map(Some),
                Err(payload) => {
                    std::mem::forget(payload);
                    Poll::Ready(None)
                }
            }
        })
        .await;
        let dropped = catch_unwind(AssertUnwindSafe(|| drop(future))).is_ok();
        match (outcome, dropped) {
            (Some(result), true) => result,
            _ => Err(AgentError::AuditFailure),
        }
    }
    pub(super) async fn wait_for_attachment_evidence(&self) -> Result<(), AgentError> {
        let result = self
            .state
            .lock()
            .expect("session lifecycle")
            .attachment_evidence
            .clone();
        let Some(mut result) = result else {
            return Ok(());
        };
        loop {
            if let Some(outcome) = result.borrow().clone() {
                return outcome;
            }
            if result.changed().await.is_err() {
                return Err(AgentError::AuditFailure);
            }
        }
    }
    pub(super) fn attachment_current(&self, generation: u64) -> bool {
        let state = self.state.lock().expect("session lifecycle");
        state.attachment_generation == generation
            && matches!(state.work_status, WorkStatus::Open)
            && matches!(state.attachment, AttachmentState::Starting { generation: current, .. } if current == generation)
    }
    pub(super) fn attachment_published(&self, generation: u64) -> bool {
        let state = self.state.lock().expect("session lifecycle");
        state.attachment_generation == generation
            && matches!(state.work_status, WorkStatus::Open)
            && matches!(state.attachment, AttachmentState::Attached { generation: current, .. } if current == generation)
    }
    pub(super) async fn run_attachment<T>(
        &self,
        generation: u64,
        operation: impl Future<Output = Result<T, AgentError>>,
    ) -> Result<T, AgentError> {
        let _transition = self.resource_transition.lock().await;
        if !self.attachment_current(generation) {
            return Err(AgentError::AttachmentAuthorizationStale);
        }
        #[cfg(test)]
        {
            let pause = self.preparation_pause.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.await;
            }
        }
        operation.await
    }
    pub(super) fn publish_attachment(
        &self,
        generation: u64,
        attached: AttachedProvider,
    ) -> Result<(), Box<AttachedProvider>> {
        let mut state = self.state.lock().expect("session lifecycle");
        if state.attachment_generation != generation
            || !matches!(state.work_status, WorkStatus::Open)
            || !matches!(state.attachment, AttachmentState::Starting { generation: current, .. } if current == generation)
        {
            return Err(Box::new(attached));
        }
        let cause = match &state.attachment {
            AttachmentState::Starting { cause, .. } => *cause,
            _ => unreachable!("matching starting attachment"),
        };
        state.provider_ready = false;
        state.automatic_recovery_ready = false;
        state.attachment = AttachmentState::Attached {
            generation,
            cause,
            provider: attached,
        };
        self.changed.send_replace(());
        Ok(())
    }
    pub(super) fn acknowledge_attachment_publication(&self, generation: u64) -> bool {
        let mut state = self.state.lock().expect("session lifecycle");
        if state.attachment_generation != generation
            || !matches!(state.work_status, WorkStatus::Open)
            || !matches!(state.attachment, AttachmentState::Attached { generation: current, .. } if current == generation)
        {
            return false;
        }
        state.provider_ready = true;
        self.changed.send_replace(());
        true
    }
    pub(super) fn fail_attachment(
        &self,
        generation: u64,
        code: AttachmentFailureCode,
        error: AgentError,
    ) -> bool {
        let mut state = self.state.lock().expect("session lifecycle");
        if state.attachment_generation == generation
            && matches!(state.attachment,
                AttachmentState::Starting { generation: current, .. }
                | AttachmentState::Attached { generation: current, .. }
                if current == generation)
        {
            state.provider_ready = false;
            let (recorded, cause) = match &state.attachment {
                AttachmentState::Starting {
                    recorded, cause, ..
                } => (*recorded, *cause),
                AttachmentState::Attached { cause, .. } => (true, *cause),
                _ => unreachable!("matching starting attachment"),
            };
            state.attachment = AttachmentState::Failed {
                failure: AttachmentFailure::new(code, generation, cause, error),
                recorded,
            };
            self.changed.send_replace(());
            true
        } else {
            false
        }
    }

    pub(super) fn retain_attachment_evidence_failure(
        &self,
        generation: u64,
        cause: AttachmentCause,
        error: AgentError,
    ) {
        let mut state = self.state.lock().expect("session lifecycle");
        state.attachment_evidence_failure = Some(AttachmentFailure::new(
            AttachmentFailureCode::Audit,
            generation,
            cause,
            error,
        ));
        self.changed.send_replace(());
    }
    pub(super) fn accept_work(self: &Arc<Self>) -> Result<WorkPermit, AgentError> {
        let work = self.accept_work_kind(WorkPhase::Active, false)?;
        #[cfg(test)]
        {
            let pause = self.work_admission_pause.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                entered.send(()).unwrap();
                release.recv().unwrap();
            }
        }
        Ok(work)
    }
    #[cfg(test)]
    pub(super) fn pause_next_work_admission(
        &self,
        entered: oneshot::Sender<()>,
        release: Receiver<()>,
    ) {
        *self.work_admission_pause.lock().unwrap() = Some((entered, release));
    }
    pub(super) fn accept_waiting_work(self: &Arc<Self>) -> Result<WorkPermit, AgentError> {
        self.accept_work_kind(WorkPhase::Waiting, false)
    }
    pub(super) fn accept_control(self: &Arc<Self>) -> Result<WorkPermit, AgentError> {
        self.accept_work_kind(WorkPhase::Active, true)
    }
    pub(super) fn claim_waiting_failure(
        &self,
        permits: &[&WorkPermit],
    ) -> Vec<Option<InvocationCancellationEvent>> {
        let mut state = self.state.lock().expect("session lifecycle");
        permits
            .iter()
            .map(|permit| {
                let work = state
                    .work
                    .get_mut(&permit.id)
                    .expect("owned queued work permit");
                if let Some(cancellation) = work.cancellation.borrow().clone() {
                    Some(cancellation)
                } else {
                    assert!(matches!(work.phase, WorkPhase::Waiting));
                    work.phase = WorkPhase::Settling;
                    None
                }
            })
            .collect()
    }
    fn accept_work_kind(
        self: &Arc<Self>,
        phase: WorkPhase,
        control: bool,
    ) -> Result<WorkPermit, AgentError> {
        let mut state = self.state.lock().expect("session lifecycle");
        // Stated as what is accepted and then negated once, rather than as two
        // negations joined: an open session takes work, and a closing one still
        // takes the waiting kind for as long as it accepts it.
        let accepted = matches!(state.work_status, WorkStatus::Open)
            || (matches!(phase, WorkPhase::Waiting) && Self::accepts_waiting(&state));
        if !accepted {
            if let Some(error) = state
                .cleanup
                .as_ref()
                .filter(|_| {
                    matches!(&state.work_status, WorkStatus::Stopping(stop)
                    if matches!(stop.recovery, RecoveryPolicy::Automatic))
                })
                .and_then(|cleanup| {
                    cleanup.result.borrow().as_ref().and_then(|report| {
                        report
                            .is_confirmed()
                            .then(|| report.audit().clone().err())
                            .flatten()
                    })
                })
            {
                return Err(error);
            }
            return Err(AgentError::Closed);
        }
        if matches!(phase, WorkPhase::Active)
            && (!state.provider_ready
                || !matches!(state.attachment, AttachmentState::Attached { .. }))
        {
            let phase = match &state.attachment {
                AttachmentState::Absent { .. } => AttachmentPhase::Absent,
                AttachmentState::Authorized(_) | AttachmentState::Abandoning { .. } => {
                    AttachmentPhase::Waiting
                }
                AttachmentState::Starting { .. } => AttachmentPhase::Starting,
                AttachmentState::Failed { failure, .. } => AttachmentPhase::Failed(failure.code()),
                AttachmentState::Attached { .. } => AttachmentPhase::Starting,
            };
            return Err(AgentError::AttachmentUnavailable(phase));
        }
        if control && !state.provider_ready {
            return Err(AgentError::AttachmentUnavailable(Self::phase(&state)));
        }
        let id = state.next_work;
        state.next_work += 1;
        let work_generation = state.work_generation;
        let (cancellation, cancellation_notice) = watch::channel(None);
        state.work.insert(
            id,
            OwnedWork {
                work_generation,
                phase,
                cancellation,
            },
        );
        Ok(WorkPermit {
            owner: self.clone(),
            id,
            binding: Mutex::new((work_generation, state.provider_generation)),
            stop: self.stop.subscribe(),
            cancellation: cancellation_notice,
        })
    }
    fn activate_waiting(&self, permit: &WorkPermit) -> Result<(), AgentError> {
        let mut state = self.state.lock().expect("session lifecycle");
        if !matches!(state.work_status, WorkStatus::Open)
            || !state.provider_ready
            || !matches!(state.attachment, AttachmentState::Attached { .. })
        {
            return Err(AgentError::AttachmentUnavailable(match &state.attachment {
                AttachmentState::Absent { .. } => AttachmentPhase::Absent,
                AttachmentState::Authorized(_) | AttachmentState::Abandoning { .. } => {
                    AttachmentPhase::Waiting
                }
                AttachmentState::Starting { .. } => AttachmentPhase::Starting,
                AttachmentState::Attached { .. } if state.provider_ready => {
                    AttachmentPhase::Attached
                }
                AttachmentState::Attached { .. } => AttachmentPhase::Starting,
                AttachmentState::Failed { failure, .. } => AttachmentPhase::Failed(failure.code()),
            }));
        }
        let work_generation = state.work_generation;
        let provider_generation = state.provider_generation;
        let work = state.work.get_mut(&permit.id).ok_or(AgentError::Closed)?;
        if !matches!(work.phase, WorkPhase::Waiting) || work.cancellation.borrow().is_some() {
            return Err(AgentError::Closed);
        }
        work.work_generation = work_generation;
        work.phase = WorkPhase::Active;
        *permit.binding.lock().expect("work binding") = (work_generation, provider_generation);
        Ok(())
    }
    pub(super) fn attached_provider(
        &self,
        permit: &WorkPermit,
    ) -> Result<AttachedProvider, AgentError> {
        let state = self.state.lock().expect("session lifecycle");
        let (work_generation, provider_generation) = permit.generations();
        if state.work_generation != work_generation
            || state.provider_generation != provider_generation
        {
            return Err(AgentError::Closed);
        }
        match &state.attachment {
            AttachmentState::Attached { provider, .. } if state.provider_ready => {
                Ok(provider.clone())
            }
            _ => Err(AgentError::AttachmentUnavailable(Self::phase(&state))),
        }
    }
    pub(super) fn active(&self) -> Option<ExecutionId> {
        self.state
            .lock()
            .expect("session lifecycle")
            .active
            .as_ref()
            .map(|(_, id)| id.clone())
    }
    pub(super) fn start_execution(&self, id: ExecutionId) -> Result<WorkGeneration, AgentError> {
        let mut state = self.state.lock().expect("session lifecycle");
        if !matches!(state.work_status, WorkStatus::Open) {
            return Err(AgentError::Closed);
        }
        let work_generation = state.work_generation;
        state.active = Some((work_generation, id));
        Ok(work_generation)
    }
    pub(super) fn finish_execution(&self, work_generation: WorkGeneration) {
        let mut state = self.state.lock().expect("session lifecycle");
        if state
            .active
            .as_ref()
            .is_some_and(|(active, _)| *active == work_generation)
        {
            state.active = None;
        }
    }
    #[cfg(test)]
    pub(super) fn pause_next_preparation(
        &self,
        entered: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    ) {
        *self.preparation_pause.lock().unwrap() = Some((entered, release));
    }
    #[cfg(test)]
    pub(super) fn notify_next_cleanup_start(&self, entered: oneshot::Sender<()>) {
        *self.cleanup_started.lock().unwrap() = Some(entered);
    }
    #[cfg(test)]
    pub(super) fn attachment_needs_cleanup(&self) -> bool {
        self.attachment.needs_cleanup()
    }
    /// Capture the work generation before polling a provider operation. Late failures only
    /// affect that work generation, never a restored or newly admitted provider generation.
    pub(super) fn work_generation(&self) -> WorkGeneration {
        self.state
            .lock()
            .expect("session lifecycle")
            .work_generation
    }
    pub(super) fn record_provider_state(
        &self,
        permit: &WorkPermit,
        provider_state: &ProviderSessionState,
    ) {
        if matches!(provider_state, ProviderSessionState::Usable) {
            return;
        }
        let mut state = self.state.lock().expect("session lifecycle");
        let (work_generation, provider_generation) = permit.generations();
        if state.provider_generation == provider_generation {
            self.apply_provider_state(&mut state, work_generation, provider_state);
        }
    }
    // Control receipts can arrive after restoration. Return their result, but
    // apply resource effects only to the attachment that admitted the control.
    pub(super) fn record_control_state(
        &self,
        permit: &WorkPermit,
        provider_state: &ProviderSessionState,
    ) {
        let mut state = self.state.lock().expect("session lifecycle");
        self.apply_control_state(&mut state, permit, provider_state);
    }
    fn apply_control_state(
        &self,
        state: &mut State,
        permit: &WorkPermit,
        provider_state: &ProviderSessionState,
    ) {
        let reported = matches!(provider_state, ProviderSessionState::CleanupReported(_));
        let (work_generation, provider_generation) = permit.generations();
        if state.provider_generation == provider_generation
            && (state.work_generation == work_generation || reported)
        {
            self.apply_provider_state(state, work_generation, provider_state);
        }
    }
    pub(super) fn start_control_cleanup(
        self: &Arc<Self>,
        permit: &WorkPermit,
    ) -> Option<CloseAttempt> {
        let mut state = self.state.lock().expect("session lifecycle");
        if state.provider_generation != permit.provider_generation_value() {
            return None;
        }
        // Applying this control's failure may already have stopped its work
        // generation. Cleanup still belongs to that same provider attachment.
        Some(self.start_stop_locked(&mut state, SessionCloseRequest::ExecutionFailed))
    }
    fn apply_provider_state(
        &self,
        state: &mut State,
        work_generation: WorkGeneration,
        provider_state: &ProviderSessionState,
    ) {
        if let ProviderSessionState::CleanupReported(report) = provider_state {
            let report = self.attachment.reconcile_cleanup(report.clone());
            if report.is_confirmed() {
                // Resource confirmation belongs to the provider attachment, even
                // when a concurrent stop already advanced the work generation.
                // Control callers checked the provider generation before this call;
                // execution reports are serialized with provider restoration.
                state.active = None;
                state.provider_ready = false;
                if matches!(state.attachment, AttachmentState::Attached { .. }) {
                    state.attachment_generation += 1;
                    state.attachment = AttachmentState::Absent { recorded: true };
                    state.automatic_recovery_ready = true;
                }
                self.notify_stop(
                    state,
                    &SessionCloseRequest::ExecutionFailed,
                    report.audit().is_err(),
                );
                if let Some(cleanup) = &state.cleanup {
                    // Pending close I/O publishes only after its final report is
                    // reconciled with this confirmation. Completed waiters and
                    // retries keep observing the same shared evidence channel.
                    if cleanup.result.borrow().is_some() {
                        cleanup.completion.send_replace(Some(report));
                    }
                } else {
                    let (completion, result) = watch::channel(Some(report));
                    state.cleanup = Some(Cleanup {
                        provider_generation: state.provider_generation,
                        completion,
                        result,
                    });
                }
                return;
            }
        }
        if matches!(provider_state, ProviderSessionState::Usable)
            || state.work_generation != work_generation
        {
            return;
        }
        // The caller starts cleanup through start_stop; this fence immediately
        // prevents another control between observing failure and starting cleanup.
        if matches!(state.work_status, WorkStatus::Open) {
            let report = match provider_state {
                ProviderSessionState::CleanupReported(report) => Some(report.clone()),
                _ => None,
            };
            let (_, result) = watch::channel(report);
            let ticket = CloseAttempt {
                id: state.work_generation.0 + 1,
                request: SessionCloseRequest::ExecutionFailed,
                result,
            };
            state.work_generation.0 += 1;
            state.work_status = WorkStatus::Blocked(Stop {
                ticket,
                finalized: false,
                recovery: RecoveryPolicy::CleanupNotStarted,
            });
            self.notify_stop(state, &SessionCloseRequest::ExecutionFailed, true);
        }
    }
    // Save each admitted owner's first causal stop before waking any operation.
    fn notify_stop(&self, state: &mut State, request: &SessionCloseRequest, include_waiting: bool) {
        let actor = match request {
            SessionCloseRequest::Explicit(actor) => Some(actor.clone()),
            _ => None,
        };
        let cancellation = InvocationCancellationEvent {
            cause: if actor.is_some() {
                SchedulingCause::SessionClosed
            } else {
                SchedulingCause::RunnerStopped
            },
            actor,
        };
        for work in state.work.values_mut() {
            if !matches!(work.phase, WorkPhase::Settling)
                && (include_waiting || !matches!(work.phase, WorkPhase::Waiting))
                && work.cancellation.borrow().is_none()
            {
                work.cancellation.send_replace(Some(cancellation.clone()));
            }
        }
        self.stop.send_replace(());
    }
    pub(super) fn block_control(&self, origin: ControlOrigin) {
        let mut state = self.state.lock().expect("session lifecycle");
        if state.provider_generation == origin.provider_generation {
            self.block_work(&mut state, origin.work_generation);
        }
    }
    pub(super) fn block(&self, work_generation: WorkGeneration) {
        let mut state = self.state.lock().expect("session lifecycle");
        self.block_work(&mut state, work_generation);
    }
    fn block_work(&self, state: &mut State, work_generation: WorkGeneration) {
        if state.work_generation != work_generation {
            return;
        }
        self.apply_provider_state(
            state,
            work_generation,
            &ProviderSessionState::CleanupRequired,
        );
        // A supervisor or storage failure cannot prove that local settlement is
        // complete merely because provider resources were released.
        if let WorkStatus::Blocked(stop) | WorkStatus::Stopping(stop) = &mut state.work_status {
            if matches!(
                stop.recovery,
                RecoveryPolicy::CleanupNotStarted | RecoveryPolicy::Automatic
            ) {
                stop.recovery = RecoveryPolicy::NeedsExplicitClose;
            }
        }
    }
    pub(super) fn start_stop(self: &Arc<Self>, request: SessionCloseRequest) -> CloseAttempt {
        let mut state = self.state.lock().expect("session lifecycle");
        self.start_stop_locked(&mut state, request)
    }
    // Old invocation evidence cannot stop a newer work generation.
    pub(super) fn start_stop_for(
        self: &Arc<Self>,
        work_generation: WorkGeneration,
        request: SessionCloseRequest,
    ) -> Option<CloseAttempt> {
        let mut state = self.state.lock().expect("session lifecycle");
        if state.work_generation != work_generation {
            return None;
        }
        Some(self.start_stop_locked(&mut state, request))
    }
    fn start_stop_locked(
        self: &Arc<Self>,
        state: &mut State,
        request: SessionCloseRequest,
    ) -> CloseAttempt {
        let fresh_queue_stop = Self::accepts_waiting(state)
            && state
                .work
                .values()
                .any(|work| work.work_generation == state.work_generation);
        let existing = match &state.work_status {
            WorkStatus::Stopping(_) if fresh_queue_stop => None,
            WorkStatus::Stopping(stop) | WorkStatus::Blocked(stop) => Some(stop.ticket.clone()),
            WorkStatus::Open => None,
        };
        let recovery = match &state.work_status {
            WorkStatus::Blocked(stop) | WorkStatus::Stopping(stop)
                if !matches!(request, SessionCloseRequest::Explicit(_)) =>
            {
                // The provider-state fence blocks admission before its owner
                // can start cleanup. Once owned cleanup starts, confirmed
                // cleanup and audit may recover it automatically. A concurrent
                // explicit close keeps ownership of its completion boundary.
                match stop.recovery {
                    RecoveryPolicy::CleanupNotStarted => RecoveryPolicy::Automatic,
                    recovery => recovery,
                }
            }
            _ if matches!(request, SessionCloseRequest::Explicit(_)) => {
                RecoveryPolicy::ExplicitClose
            }
            _ => RecoveryPolicy::Automatic,
        };
        let first_request = existing
            .as_ref()
            .map_or_else(|| request.clone(), |ticket| ticket.request.clone());
        let stop_id = existing
            .as_ref()
            .map_or(state.work_generation.0 + 1, |ticket| ticket.id);
        if existing.is_none() {
            state.work_generation.0 += 1;
            let recorded = match &state.attachment {
                AttachmentState::Absent { recorded }
                | AttachmentState::Abandoning { recorded, .. }
                | AttachmentState::Starting { recorded, .. }
                | AttachmentState::Failed { recorded, .. } => *recorded,
                AttachmentState::Authorized(authority) => {
                    !matches!(authority.cause, AttachmentCause::Initial)
                }
                AttachmentState::Attached { .. } => true,
            };
            let retained_failure = match &state.attachment {
                AttachmentState::Failed { failure, .. } => Some(failure.clone()),
                _ => None,
            };
            if let AttachmentState::Authorized(authority) = &state.attachment {
                authority.cancelled.send_replace(true);
            }
            if let AttachmentState::Starting { result, .. } = &state.attachment {
                result.send_replace(Some(Err(AgentError::Closed)));
            }
            state.attachment_generation += 1;
            state.attachment = match retained_failure {
                Some(failure) => AttachmentState::Failed { failure, recorded },
                None => AttachmentState::Absent { recorded },
            };
        }
        if let SessionCloseRequest::Explicit(actor) = &request {
            self.close.send_replace(Some(actor.clone()));
        }
        // Joining cleanup does not reuse its cause for newly affected work.
        // notify_stop preserves first causes for owners that already stopped.
        self.notify_stop(
            state,
            &request,
            !matches!(recovery, RecoveryPolicy::Automatic),
        );
        state.provider_ready = false;
        let shared = state.cleanup.as_ref().and_then(|cleanup| {
            let reusable = cleanup.provider_generation == state.provider_generation
                && match cleanup.result.borrow().as_ref() {
                    Some(report) => report.is_confirmed(),
                    None => cleanup.result.has_changed().is_ok(),
                };
            reusable.then(|| cleanup.result.clone())
        });
        let result = if let Some(result) = shared {
            result
        } else {
            let (completion, result) = watch::channel(None);
            let provider_generation = state.provider_generation;
            state.cleanup = Some(Cleanup {
                provider_generation,
                completion: completion.clone(),
                result: result.clone(),
            });
            let owner = self.clone();
            tokio::spawn(async move {
                #[cfg(test)]
                if let Some(entered) = owner.cleanup_started.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                // Admission closes synchronously above. Resource ownership must
                // wait for an in-progress restoration to arm its new attachment.
                // Capture the cause after arm too, because arm starts a new owner.
                let _transition = owner.resource_transition.lock().await;
                owner.attachment.request_cleanup(request);
                let report = owner.attachment.cleanup().await;
                // Publish under the same state lock used by operation reports:
                // confirmation cannot arrive between reconciliation and publication.
                let _state = owner.state.lock().expect("session lifecycle");
                let report = owner.attachment.reconcile_cleanup(report);
                completion.send_replace(Some(report));
                owner.changed.send_replace(());
            });
            result
        };
        let ticket = CloseAttempt {
            id: stop_id,
            request: first_request,
            result,
        };
        state.work_status = WorkStatus::Stopping(Stop {
            ticket: ticket.clone(),
            finalized: false,
            recovery,
        });
        ticket
    }
    /// Called only after queue cancellation and evidence settlement have completed.
    /// Work permits delay reopening until every previously admitted owner retires.
    pub(super) async fn finalize_stop(
        &self,
        ticket: &CloseAttempt,
        report: &CleanupReport,
    ) -> CleanupReport {
        let _transition = self.resource_transition.lock().await;
        let mut state = self.state.lock().expect("session lifecycle");
        let current = matches!(&state.work_status, WorkStatus::Stopping(stop) | WorkStatus::Blocked(stop) if stop.ticket.id == ticket.id);
        if !current {
            return report.clone();
        }
        // A concurrent retry or operation may have supplied newer evidence while
        // this waiter was settling work. Never republish its earlier snapshot.
        let report = state
            .cleanup
            .as_ref()
            .and_then(|cleanup| cleanup.result.borrow().clone())
            .unwrap_or_else(|| report.clone());
        let report = self.attachment.reconcile_cleanup(report);
        let stop = match &mut state.work_status {
            WorkStatus::Stopping(stop) | WorkStatus::Blocked(stop)
                if stop.ticket.id == ticket.id =>
            {
                stop
            }
            _ => return report,
        };
        stop.finalized = true;
        let updated = Stop {
            ticket: stop.ticket.clone(),
            finalized: true,
            recovery: stop.recovery,
        };
        state.work_status = if report.is_confirmed() {
            WorkStatus::Stopping(updated)
        } else {
            WorkStatus::Blocked(updated)
        };
        if let Some(cleanup) = &state.cleanup {
            cleanup.completion.send_replace(Some(report.clone()));
        }
        if report.is_confirmed() {
            state.active = None;
        }
        if !report.is_confirmed() || report.audit().is_err() {
            // Final cleanup failure removes the promised restoration boundary.
            // Waiting receipts must settle instead of waiting for a future close.
            self.notify_stop(&mut state, &ticket.request, true);
        }
        self.maybe_reopen(&mut state);
        report
    }
    pub(super) fn accepts_queued(&self) -> bool {
        let state = self.state.lock().unwrap();
        matches!(state.work_status, WorkStatus::Open) || Self::accepts_waiting(&state)
    }
    fn accepts_waiting(state: &State) -> bool {
        matches!(&state.work_status, WorkStatus::Stopping(stop) if stop.finalized
            && matches!(stop.recovery, RecoveryPolicy::Automatic)
            && !matches!(stop.ticket.request, SessionCloseRequest::Explicit(_))
            && stop.ticket.result.borrow().as_ref().is_some_and(|report| report.is_confirmed() && report.audit().is_ok()))
    }
    fn maybe_reopen(&self, state: &mut State) {
        let ready = match &state.work_status {
            WorkStatus::Stopping(stop)
                if stop.finalized
                    && matches!(
                        stop.recovery,
                        RecoveryPolicy::Automatic | RecoveryPolicy::ExplicitClose
                    )
                    && state.work.values().all(|work| {
                        matches!(work.phase, WorkPhase::Waiting)
                            || work.work_generation == state.work_generation
                    }) =>
            {
                stop.ticket
                    .result
                    .borrow()
                    .as_ref()
                    .is_some_and(|report| report.is_confirmed() && report.audit().is_ok())
            }
            _ => false,
        };
        if ready {
            let automatic = matches!(
                &state.work_status,
                WorkStatus::Stopping(stop)
                    if matches!(stop.recovery, RecoveryPolicy::Automatic)
            );
            state.work_status = WorkStatus::Open;
            state.active = None;
            state.automatic_recovery_ready = automatic;
            self.close.send_replace(None);
        }
    }
    pub(super) async fn wait_for_work(&self) {
        let mut changed = self.changed.subscribe();
        loop {
            if self
                .state
                .lock()
                .expect("session lifecycle")
                .work
                .is_empty()
            {
                return;
            }
            let _ = changed.changed().await;
        }
    }
    pub(super) async fn run_control<T>(
        &self,
        permit: &WorkPermit,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
    ) -> Result<T, AgentError> {
        self.run_provider_operation(permit, operation, false)
            .await
            .map_err(ProviderOperationFailure::into_error)
    }
    pub(super) async fn run_control_observed<T>(
        &self,
        permit: &WorkPermit,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
    ) -> Result<T, ProviderOperationFailure> {
        self.run_provider_operation(permit, operation, false).await
    }
    pub(super) async fn run_preparation<T>(
        &self,
        permit: &WorkPermit,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
    ) -> Result<T, AgentError> {
        let value = self
            .run_provider_operation(permit, operation, true)
            .await
            .map_err(ProviderOperationFailure::into_error)?;
        let mut state = self.state.lock().expect("session lifecycle");
        let (work_generation, provider_generation) = permit.generations();
        if state.work_generation != work_generation
            || state.provider_generation != provider_generation
            || !matches!(state.work_status, WorkStatus::Open)
            || state.cleanup.is_some()
        {
            return Err(AgentError::Closed);
        }
        state.provider_ready = true;
        Ok(value)
    }
    async fn run_provider_operation<T>(
        &self,
        permit: &WorkPermit,
        operation: impl Future<Output = Result<T, ProviderOperationFailure>>,
        preparation: bool,
    ) -> Result<T, ProviderOperationFailure> {
        let mut stop = permit.stop.clone();
        tokio::pin!(operation);
        let mut started = false;
        let stopped = stop.changed();
        tokio::pin!(stopped);
        std::future::poll_fn(|cx| {
            match self.poll_control(permit, operation.as_mut(), cx, preparation, &mut started) {
                Poll::Ready(result) => Poll::Ready(result),
                Poll::Pending if stopped.as_mut().poll(cx).is_ready() => {
                    // Shutdown may have won the lock just after the first poll.
                    // Give a concurrently delivered receipt the same final check.
                    match self.poll_control(
                        permit,
                        operation.as_mut(),
                        cx,
                        preparation,
                        &mut started,
                    ) {
                        Poll::Ready(result) => Poll::Ready(result),
                        Poll::Pending => Poll::Ready(Err(ProviderOperationFailure::new(
                            AgentError::Closed,
                            ProviderSessionState::CleanupRequired,
                        ))),
                    }
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }
    fn poll_control<T>(
        &self,
        permit: &WorkPermit,
        operation: Pin<&mut impl Future<Output = Result<T, ProviderOperationFailure>>>,
        cx: &mut Context<'_>,
        preparation: bool,
        started: &mut bool,
    ) -> Poll<Result<T, ProviderOperationFailure>> {
        let mut state = self.state.lock().expect("session lifecycle");
        let (work_generation, provider_generation) = permit.generations();
        let stopped = state.work_generation != work_generation
            || state.provider_generation != provider_generation
            || (!preparation && !state.provider_ready)
            || !matches!(state.work_status, WorkStatus::Open);
        // Never start provider work after its admission closes. Once started,
        // retain an already-ready acknowledgement before honoring a concurrent
        // stop; otherwise a confirmed external effect would be lost.
        if stopped && (!*started || preparation) {
            return Poll::Ready(Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::CleanupRequired,
            )));
        }
        *started = true;
        let result = catch_unwind(AssertUnwindSafe(|| {
            if stopped {
                // One final poll must see a ready acknowledgement even when this
                // task has spent its cooperative budget. Never await after stop.
                let final_poll = tokio::task::unconstrained(operation);
                tokio::pin!(final_poll);
                final_poll.poll(cx)
            } else {
                operation.poll(cx)
            }
        }))
        .unwrap_or_else(|_| {
            Poll::Ready(Err(ProviderOperationFailure::new(
                AgentError::Protocol("provider control panicked".into()),
                ProviderSessionState::CleanupRequired,
            )))
        });
        match result {
            Poll::Ready(Err(failure)) => {
                self.apply_control_state(&mut state, permit, failure.session_state());
                Poll::Ready(Err(failure))
            }
            Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
            Poll::Pending if stopped => Poll::Ready(Err(ProviderOperationFailure::new(
                AgentError::Closed,
                ProviderSessionState::CleanupRequired,
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}
