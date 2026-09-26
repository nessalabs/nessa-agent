//! Keeps the storage writer lease owned until the attached provider confirms cleanup.
//! Explicit retry and automatic drop cleanup share the same resource owner.
//! The first shutdown request remains authoritative until cleanup is confirmed;
//! a subsequently armed attachment starts with its own handle-drop cause.

use crate::application::agent_execution::{
    agents::AgentError,
    providers::{
        CleanupReport, CloseOutcome, ExecutionEventStream, ProviderCleanup, ProviderSession,
        ResourceCleanup, SessionCloseRequest,
    },
    sessions::SessionStorageLease,
};
use std::{
    future::poll_fn,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex as StateMutex,
    },
    task::Poll,
    time::Duration,
};
use tokio::{runtime::Handle, sync::Mutex, time::sleep};

pub(crate) struct AttachmentLease {
    state: Mutex<AttachmentState>,
    // Operation reports can confirm release while close is still awaiting I/O.
    // This synchronous evidence also governs final handle drop.
    cleanup_report: StateMutex<Option<CleanupReport>>,
    pending: AtomicBool,
    // Provider opens still running. Whatever an open launched is armed above
    // before its count is released, so the two facts leave no gap.
    opening: AtomicUsize,
    drop_reason: StateMutex<CleanupReason>,
    runtime: Handle,
}
struct CleanupReason {
    request: SessionCloseRequest,
    started: bool,
}
impl CleanupReason {
    fn new(request: SessionCloseRequest) -> Self {
        Self {
            request,
            started: false,
        }
    }
}
enum AttachmentState {
    Empty,
    Attached(Resources),
    // An adapter panicked before transferring a cleanup handle. No retry can prove release.
    UnknownOpen(Arc<dyn SessionStorageLease>),
    // Releasing physical resources does not acknowledge failed audit delivery.
    Cleaned(CleanupReport),
}
#[derive(Clone)]
struct Resources {
    target: CleanupTarget,
    // This clone outlives the manager if attachment cleanup remains uncertain.
    _lease: Arc<dyn SessionStorageLease>,
    _events: Option<Arc<Mutex<Box<dyn ExecutionEventStream>>>>,
}
#[derive(Clone)]
enum CleanupTarget {
    Session(ProviderSession),
    FailedOpen(Arc<dyn ProviderCleanup>),
}
impl Resources {
    async fn cleanup(&self, reason: SessionCloseRequest) -> CleanupReport {
        let operation = catch_unwind(AssertUnwindSafe(|| match &self.target {
            CleanupTarget::Session(session) => session.shutdown(reason),
            CleanupTarget::FailedOpen(cleanup) => cleanup.retry_cleanup(),
        }));
        let mut operation = match operation {
            Ok(operation) => operation,
            Err(payload) => {
                std::mem::forget(payload);
                return CleanupReport::unconfirmed(AgentError::CleanupUncertain);
            }
        };
        let result = poll_fn(|context| {
            match catch_unwind(AssertUnwindSafe(|| operation.as_mut().poll(context))) {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(report)) => Poll::Ready(report),
                Err(payload) => {
                    std::mem::forget(payload);
                    Poll::Ready(CleanupReport::unconfirmed(AgentError::CleanupUncertain))
                }
            }
        })
        .await;
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(operation))) {
            std::mem::forget(payload);
            // Future destruction cannot erase a report already returned by poll.
            // Preserve physical and audit evidence and record destruction separately.
            let completion_failure = match result.completion_failure().cloned() {
                Some(first_error) => AgentError::MultipleOperationFailures {
                    first_error: Box::new(first_error),
                    subsequent_error: Box::new(AgentError::CleanupUncertain),
                }
                .bounded(),
                None => AgentError::CleanupUncertain,
            };
            return result.with_completion_failure(Some(completion_failure));
        }
        result
    }
}
/// One provider open in flight, counted by [`AttachmentLease::opening`].
pub(crate) struct OpenInFlight<'a>(&'a AtomicUsize);
impl Drop for OpenInFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl AttachmentLease {
    pub(crate) fn empty() -> Self {
        Self {
            state: Mutex::new(AttachmentState::Empty),
            cleanup_report: StateMutex::new(None),
            pending: AtomicBool::new(false),
            opening: AtomicUsize::new(0),
            drop_reason: StateMutex::new(CleanupReason::new(SessionCloseRequest::SessionFailed)),
            runtime: Handle::current(),
        }
    }
    pub(crate) async fn arm(
        &self,
        lease: Arc<dyn SessionStorageLease>,
        session: ProviderSession,
        events: Arc<Mutex<Box<dyn ExecutionEventStream>>>,
    ) {
        let mut state = self.state.lock().await;
        let mut evidence = self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence");
        if matches!(*state, AttachmentState::Empty | AttachmentState::Cleaned(_))
            || evidence.as_ref().is_some_and(CleanupReport::is_confirmed)
        {
            *evidence = None;
            *state = AttachmentState::Attached(Resources {
                target: CleanupTarget::Session(session),
                _lease: lease,
                _events: Some(events),
            });
            *self.drop_reason.lock().expect("attachment drop reason") =
                CleanupReason::new(SessionCloseRequest::SessionHandlesDropped);
            self.pending.store(true, Ordering::SeqCst);
        }
    }
    pub(crate) async fn arm_failed_open(
        &self,
        lease: Arc<dyn SessionStorageLease>,
        cleanup: Arc<dyn ProviderCleanup>,
        report: CleanupReport,
    ) {
        self.arm_target(lease, CleanupTarget::FailedOpen(cleanup))
            .await;
        self.reconcile_cleanup(report);
    }
    pub(crate) async fn arm_unknown_open(&self, lease: Arc<dyn SessionStorageLease>) {
        let mut state = self.state.lock().await;
        *state = AttachmentState::UnknownOpen(lease);
        self.pending.store(true, Ordering::SeqCst);
    }
    async fn arm_target(&self, lease: Arc<dyn SessionStorageLease>, target: CleanupTarget) {
        let mut state = self.state.lock().await;
        *self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence") = None;
        *state = AttachmentState::Attached(Resources {
            target,
            _lease: lease,
            _events: None,
        });
        self.pending.store(true, Ordering::SeqCst);
    }
    /// Capture the first shutdown request before provider I/O. Later attempts
    /// retire the same attachment and must preserve its cause and known initiator.
    pub(crate) fn request_cleanup(&self, request: SessionCloseRequest) -> SessionCloseRequest {
        let mut reason = self.drop_reason.lock().expect("attachment drop reason");
        if !reason.started {
            reason.request = request;
            reason.started = true;
        }
        reason.request.clone()
    }
    pub(crate) fn confirm_cleanup(&self, report: &CleanupReport) -> CleanupReport {
        debug_assert!(report.is_confirmed());
        self.reconcile_cleanup(report.clone())
    }
    pub(crate) fn reconcile_cleanup(&self, report: CleanupReport) -> CleanupReport {
        let mut evidence = self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence");
        let report = match evidence.as_ref() {
            Some(prior) => Self::merge_cleanup(prior, &report),
            None => report,
        };
        *evidence = Some(report.clone());
        if report.is_confirmed() {
            self.pending.store(false, Ordering::SeqCst);
        }
        report
    }
    pub(crate) fn publish_final_cleanup(&self, report: CleanupReport) -> CleanupReport {
        *self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence") = Some(report.clone());
        self.pending.store(!report.is_confirmed(), Ordering::SeqCst);
        report
    }
    fn record_cleanup_attempt(&self, report: CleanupReport) -> CleanupReport {
        let mut evidence = self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence");
        // A real retry can acknowledge an earlier failed audit while resources
        // remain owned. Independent operation confirmation is different: once
        // confirmed, a competing close result cannot revoke that known release.
        let report = match evidence.as_ref().filter(|prior| prior.is_confirmed()) {
            Some(prior) => Self::merge_cleanup(prior, &report),
            None => report,
        };
        *evidence = Some(report.clone());
        if report.is_confirmed() {
            self.pending.store(false, Ordering::SeqCst);
        }
        report
    }
    fn record_failed_open_cleanup(&self, report: CleanupReport) -> CleanupReport {
        let mut evidence = self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence");
        let report = match evidence.as_ref() {
            Some(prior) => Self::merge_cleanup(prior, &report),
            None => report,
        };
        *evidence = Some(report.clone());
        if report.is_confirmed() {
            self.pending.store(false, Ordering::SeqCst);
        }
        report
    }
    fn merge_cleanup(prior: &CleanupReport, report: &CleanupReport) -> CleanupReport {
        if prior == report {
            return prior.clone();
        }
        let audit = match (prior.audit(), report.audit()) {
            (Err(first), Err(next)) => Err(Self::combine_failures(first.clone(), next.clone())),
            (Err(error), _) | (_, Err(error)) => Err(error.clone()),
            _ => Ok(()),
        };
        let mut failure = prior.operation_failure().cloned();
        let mut completion_failure = prior.completion_failure().cloned();
        let superseded = match prior.resources() {
            ResourceCleanup::Unconfirmed(error) if report.is_confirmed() => Some(error),
            _ => None,
        };
        for error in report
            .operation_failure()
            .into_iter()
            .chain(superseded)
            .chain(match report.resources() {
                ResourceCleanup::Unconfirmed(error) if prior.is_confirmed() => Some(error),
                _ => None,
            })
        {
            failure = Some(match failure {
                Some(first) => Self::combine_failures(first, error.clone()),
                None => error.clone(),
            });
        }
        if let Some(error) = report.completion_failure() {
            completion_failure = Some(match completion_failure {
                Some(first) => Self::combine_failures(first, error.clone()),
                None => error.clone(),
            });
        }
        let resources = match (prior.resources(), report.resources()) {
            (ResourceCleanup::Confirmed(_), _) => prior.resources().clone(),
            (_, ResourceCleanup::Confirmed(_)) => report.resources().clone(),
            (ResourceCleanup::ReleasePending { .. }, ResourceCleanup::Unconfirmed(_)) => {
                prior.resources().clone()
            }
            _ => report.resources().clone(),
        };
        CleanupReport::new(resources, audit)
            .with_operation_failure(failure)
            .with_completion_failure(completion_failure)
    }
    // Re-observing a report adds no new failure. Flatten only this diagnostic
    // grouping; other typed errors retain their own lifecycle meaning intact.
    fn combine_failures(first: AgentError, next: AgentError) -> AgentError {
        let mut pending = vec![next, first];
        let mut failures = Vec::new();
        while let Some(error) = pending.pop() {
            match error {
                AgentError::MultipleOperationFailures {
                    first_error,
                    subsequent_error,
                } => {
                    pending.push(*subsequent_error);
                    pending.push(*first_error);
                }
                AgentError::DiagnosticLimit => return AgentError::DiagnosticLimit,
                error if !failures.contains(&error) => failures.push(error),
                _ => {}
            }
        }
        let mut failures = failures.into_iter();
        let first = failures.next().expect("at least one cleanup failure");
        failures
            .fold(first, |first, next| AgentError::MultipleOperationFailures {
                first_error: Box::new(first),
                subsequent_error: Box::new(next),
            })
            .bounded()
    }
    pub(crate) fn needs_cleanup(&self) -> bool {
        self.pending.load(Ordering::SeqCst)
    }
    /// Count one provider open until the returned guard is dropped, including
    /// when the open is cancelled or panics.
    pub(crate) fn opening(&self) -> OpenInFlight<'_> {
        self.opening.fetch_add(1, Ordering::SeqCst);
        OpenInFlight(&self.opening)
    }
    pub(crate) fn open_in_flight(&self) -> bool {
        self.opening.load(Ordering::SeqCst) > 0
    }
    pub(crate) async fn cleanup(&self) -> CleanupReport {
        let mut state = self.state.lock().await;
        if let Some(report) = self
            .cleanup_report
            .lock()
            .expect("attachment cleanup evidence")
            .as_ref()
            .filter(|report| report.is_confirmed())
            .cloned()
        {
            return report;
        }
        let owned = match &*state {
            AttachmentState::Empty => {
                return CleanupReport::confirmed(CloseOutcome { forced: false })
            }
            AttachmentState::Attached(owned) => owned,
            AttachmentState::Cleaned(report) => return report.clone(),
            AttachmentState::UnknownOpen(_) => {
                return CleanupReport::unconfirmed(AgentError::CleanupUncertain)
            }
        };
        let reason = {
            let mut reason = self.drop_reason.lock().expect("attachment drop reason");
            reason.started = true;
            reason.request.clone()
        };
        let failed_open = matches!(&owned.target, CleanupTarget::FailedOpen(_));
        let attempted = owned.cleanup(reason).await;
        let result = if failed_open {
            self.record_failed_open_cleanup(attempted)
        } else {
            self.record_cleanup_attempt(attempted)
        };
        if result.is_confirmed() {
            *state = AttachmentState::Cleaned(result.clone());
            self.pending.store(false, Ordering::SeqCst);
        }
        result
    }
}
// Runtime shutdown can cancel the last cleanup task. Without a surviving retry
// owner, keep the writer fenced for this process rather than claiming release.
struct CleanupOwner(Option<Resources>);
impl Drop for CleanupOwner {
    fn drop(&mut self) {
        if let Some(resources) = self.0.take() {
            std::mem::forget(resources);
        }
    }
}
impl Drop for AttachmentLease {
    fn drop(&mut self) {
        if self
            .cleanup_report
            .get_mut()
            .expect("attachment cleanup evidence")
            .as_ref()
            .is_some_and(CleanupReport::is_confirmed)
        {
            return;
        }
        if let AttachmentState::UnknownOpen(lease) = self.state.get_mut() {
            // No cleanup capability was transferred. Preserve exclusion for this
            // process rather than treating caller/error/runtime drop as proof.
            std::mem::forget(lease.clone());
            return;
        }
        if matches!(self.state.get_mut(), AttachmentState::Empty) {
            return;
        }
        let AttachmentState::Attached(resources) = self.state.get_mut() else {
            return;
        };
        // The task's clones retain the same resource owners after this value drops.
        let mut owner = CleanupOwner(Some(resources.clone()));
        let reason = self
            .drop_reason
            .get_mut()
            .expect("attachment drop reason")
            .request
            .clone();
        // The task owns plain resources, not another AttachmentLease. Runtime
        // shutdown therefore cannot recursively spawn cleanup from a dropped task.
        drop(self.runtime.spawn(async move {
            let mut backoff = Duration::from_millis(100);
            loop {
                let report = owner.0.as_ref().expect("cleanup owner").cleanup(reason.clone()).await;
                if report.is_confirmed() {
                    if let Err(error) = report.into_result() {
                        tracing::warn!(%error, "attachment cleanup completed with evidence failure");
                    }
                    owner.0.take();
                    break;
                }
                if let Err(error) = report.into_result() {
                    tracing::warn!(%error, "retaining session storage lease until attachment cleanup is confirmed");
                }
                sleep(backoff).await;
                backoff = backoff.saturating_mul(2).min(Duration::from_secs(5));
            }
            // Releasing the owned resources releases this last protective lease clone.
        }));
    }
}
