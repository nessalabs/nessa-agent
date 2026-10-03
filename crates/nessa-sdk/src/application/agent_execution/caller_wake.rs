//! Keeps a panic raised by a caller's `Waker` inside the caller's own wait.
//!
//! A public SDK future polled on the caller's task registers the caller's
//! waker with whatever it waits on: a result channel, the event broadcast, a
//! Tokio mutex. An SDK-owned task (the queue runner, an attachment task, a
//! close) then calls that waker synchronously when it publishes or releases.
//! The safe `std::task::Wake` trait does not forbid a waker to panic, so
//! without this boundary the caller's panic would unwind the SDK's task.
//!
//! The ownership table, its sites and tests are in "Caller wakers" in
//! docs/agent_execution/lifecycle.md.
use crate::domain::agent_execution::{executions::ExecutionId, sessions::SessionId};
use std::{
    fmt,
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::pin,
    sync::Arc,
    task::{Context, Wake, Waker},
};

/// The public wait whose caller supplied the waker. Logged when it panics.
pub(crate) enum CallerWaiter {
    /// `QueuedInvocation::wait` for this queued input.
    Receipt(ExecutionId),
    /// `AgentEvents::next` on this session's Agent.
    Events(SessionId),
    /// `AttachmentCancellation::wait` for this attachment generation.
    AttachmentCancellation { generation: u64 },
    /// `ProviderOpenControl::wait` for a provider opening this session, if
    /// the request names one.
    ProviderOpenStop(Option<SessionId>),
    /// `Agent::queued_ids` on this session's Agent.
    QueuedIds(SessionId),
    /// `Agent::idle_for_approval_change` on this session's Agent.
    IdleForApprovalChange(SessionId),
    /// `Agent::set_approval_mode` on this session's Agent.
    ApprovalModeChange(SessionId),
    /// `SessionManager::snapshot` for this session.
    CommittedSnapshot(SessionId),
    /// `Agent::invoke` of this input.
    Invocation(ExecutionId),
    /// `Agent::enqueue` or `Agent::enqueue_steering` of this input.
    Admission(ExecutionId),
    /// `Agent::steer` of this input.
    Steering(ExecutionId),
    /// `Agent::reorder_queued` on this session's Agent.
    QueueReorder(SessionId),
    /// `Agent::remove_queued` of this input.
    QueueRemoval(ExecutionId),
    /// `Agent::close` of this session's Agent.
    Close(SessionId),
    /// `Agent::set_effort_level` on this session's Agent.
    EffortLevelChange(SessionId),
    /// `Agent::answer_permission` on this session's Agent.
    PermissionAnswer(SessionId),
    /// `Agent::cancel_permission` on this session's Agent.
    PermissionCancellation(SessionId),
    /// `Agent::answer_question` on this session's Agent.
    QuestionAnswer(SessionId),
}

impl fmt::Display for CallerWaiter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Receipt(id) => write!(formatter, "queued receipt {}", id.as_str()),
            Self::Events(session) => {
                write!(formatter, "event stream of session {}", session.as_str())
            }
            Self::AttachmentCancellation { generation } => {
                write!(
                    formatter,
                    "cancellation of attachment generation {generation}"
                )
            }
            Self::ProviderOpenStop(Some(session)) => {
                write!(
                    formatter,
                    "provider open stop for session {}",
                    session.as_str()
                )
            }
            Self::ProviderOpenStop(None) => formatter.write_str("provider open stop"),
            Self::QueuedIds(session) => {
                write!(formatter, "queued ids of session {}", session.as_str())
            }
            Self::IdleForApprovalChange(session) => {
                write!(formatter, "idle check of session {}", session.as_str())
            }
            Self::ApprovalModeChange(session) => {
                write!(
                    formatter,
                    "approval mode change of session {}",
                    session.as_str()
                )
            }
            Self::CommittedSnapshot(session) => {
                write!(
                    formatter,
                    "committed snapshot of session {}",
                    session.as_str()
                )
            }
            Self::Invocation(id) => write!(formatter, "invocation {}", id.as_str()),
            Self::Admission(id) => write!(formatter, "admission of {}", id.as_str()),
            Self::Steering(id) => write!(formatter, "steering of {}", id.as_str()),
            Self::QueueReorder(session) => {
                write!(formatter, "queue reorder of session {}", session.as_str())
            }
            Self::QueueRemoval(id) => write!(formatter, "removal of queued {}", id.as_str()),
            Self::Close(session) => write!(formatter, "close of session {}", session.as_str()),
            Self::EffortLevelChange(session) => {
                write!(
                    formatter,
                    "effort level change of session {}",
                    session.as_str()
                )
            }
            Self::PermissionAnswer(session) => {
                write!(
                    formatter,
                    "permission answer in session {}",
                    session.as_str()
                )
            }
            Self::PermissionCancellation(session) => {
                write!(
                    formatter,
                    "permission cancellation in session {}",
                    session.as_str()
                )
            }
            Self::QuestionAnswer(session) => {
                write!(formatter, "question answer in session {}", session.as_str())
            }
        }
    }
}

/// Polls `future` with a waker that wakes the caller's waker inside
/// `catch_unwind`, so a caller's waker panic stays out of whichever task
/// wakes it. The wait loses that one wake; its result is unaffected.
pub(crate) async fn contain_caller_wake<F: Future>(waiter: CallerWaiter, future: F) -> F::Output {
    let waiter = Arc::new(waiter);
    let mut future = pin!(future);
    poll_fn(|context| {
        let contained = Waker::from(Arc::new(ContainedWake {
            caller: context.waker().clone(),
            waiter: waiter.clone(),
        }));
        future.as_mut().poll(&mut Context::from_waker(&contained))
    })
    .await
}

struct ContainedWake {
    caller: Waker,
    waiter: Arc<CallerWaiter>,
}
impl Wake for ContainedWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| self.caller.wake_by_ref())) {
            tracing::warn!(waiter = %self.waiter, "a caller's waker panicked; its wait keeps its result");
            // The payload is caller data too; its drop may panic. Forget a
            // second payload rather than drop it.
            let _ = catch_unwind(AssertUnwindSafe(|| drop(payload))).map_err(std::mem::forget);
        }
    }
}
