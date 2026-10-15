//! Bounds Agent scheduling and attachment audit observations, not physical writes.
//! The adapter owns started I/O; caller-specific panic policy stays with the caller.
use super::AgentError;
use crate::application::agent_execution::{
    executions::{ExecutionAudit, ExecutionAuditRecord},
    sessions::MessageCommitClock,
};
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

pub(super) const AUDIT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(30);

// Reorder owns panic recovery; an unacknowledged write must not erase that cause.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AuditObservationFailure {
    Unacknowledged(AgentError),
    Panicked,
}
impl AuditObservationFailure {
    pub(super) fn into_agent_error(self) -> AgentError {
        match self {
            Self::Unacknowledged(error) => error,
            Self::Panicked => AgentError::AuditFailure,
        }
    }
}

pub(super) struct AuditObserver {
    sink: Arc<dyn ExecutionAudit>,
    clock: Arc<dyn MessageCommitClock>,
}
impl AuditObserver {
    pub(super) fn new(sink: Arc<dyn ExecutionAudit>, clock: Arc<dyn MessageCommitClock>) -> Self {
        Self { sink, clock }
    }

    pub(super) async fn record(
        &self,
        record: ExecutionAuditRecord,
        operation: &'static str,
        on_panic: impl Fn(),
    ) -> Result<(), AuditObservationFailure> {
        let result = async {
            let deadline = contain_construction(&on_panic, || {
                self.clock.now().saturating_add(AUDIT_OBSERVATION_TIMEOUT)
            })?;
            let timer = contain_construction(&on_panic, || self.clock.sleep_until(deadline))?;
            let mut timer = GuardedObservation {
                future: Some(timer),
                on_panic: &on_panic,
            };
            let future = contain_construction(&on_panic, || self.sink.record(record))?;
            let mut observation = GuardedObservation {
                future: Some(future),
                on_panic: &on_panic,
            };
            let outcome = poll_fn(|context| {
                match observation.poll(context) {
                    Poll::Ready(result) => return Poll::Ready(result),
                    Poll::Pending => {}
                }
                match timer.poll(context) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(Err(AgentError::AuditFailure))),
                    Poll::Ready(Err(())) => Poll::Ready(Err(())),
                    Poll::Pending => Poll::Pending,
                }
            })
            .await;
            let observation_dropped = observation.finish();
            let timer_dropped = timer.finish();
            match (outcome, observation_dropped, timer_dropped) {
                (Ok(result), true, true) => result.map_err(AuditObservationFailure::Unacknowledged),
                _ => Err(AuditObservationFailure::Panicked),
            }
        }
        .await;
        if let Err(error) = &result {
            let failure = match error {
                AuditObservationFailure::Unacknowledged(_) => "unacknowledged",
                AuditObservationFailure::Panicked => "panicked",
            };
            tracing::warn!(operation, failure, "audit observation failed");
        }
        result
    }
}

fn contain_construction<T>(
    on_panic: &impl Fn(),
    construct: impl FnOnce() -> T,
) -> Result<T, AuditObservationFailure> {
    match catch_unwind(AssertUnwindSafe(construct)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            std::mem::forget(payload);
            on_panic();
            Err(AuditObservationFailure::Panicked)
        }
    }
}

// Keep the returned future outside its protected poll; fence before the SDK
// drops it after a poll fault. Port-internal unwind cleanup stays with the port.
struct GuardedObservation<F: ?Sized, P: Fn()> {
    future: Option<Pin<Box<F>>>,
    on_panic: P,
}
impl<F: ?Sized, P: Fn()> GuardedObservation<F, P> {
    fn finish(&mut self) -> bool {
        match catch_unwind(AssertUnwindSafe(|| drop(self.future.take()))) {
            Ok(()) => true,
            Err(payload) => {
                std::mem::forget(payload);
                (self.on_panic)();
                false
            }
        }
    }
}
impl<F: Future + ?Sized, P: Fn()> GuardedObservation<F, P> {
    fn poll(&mut self, context: &mut Context<'_>) -> Poll<Result<F::Output, ()>> {
        match catch_unwind(AssertUnwindSafe(|| {
            self.future
                .as_mut()
                .expect("owned audit observation")
                .as_mut()
                .poll(context)
        })) {
            Ok(poll) => poll.map(Ok),
            Err(payload) => {
                std::mem::forget(payload);
                (self.on_panic)();
                Poll::Ready(Err(()))
            }
        }
    }
}
impl<F: ?Sized, P: Fn()> Drop for GuardedObservation<F, P> {
    fn drop(&mut self) {
        self.finish();
    }
}
