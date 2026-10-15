//! A review selection is acknowledged before it may enter a socket's UI queue.
//!
//! Admission bounds owned attempts across sockets. After Cedar answers, `record`
//! transfers the complete selection to an independent task synchronously. A
//! publisher may disappear without cancelling that attempt. Its observer has a
//! deadline; physical persistence remains the adapter's responsibility. Host
//! shutdown closes admission and observes every admitted selection, retaining the
//! first unacknowledged failure instead of inferring success from task absence.
use super::AccessDecision;
use crate::conversation::domain::Conversation;
use futures_util::FutureExt;
use nessa_auth::{
    application::ports::AccessSnapshot,
    domain::{AuthContext, MembershipRole, OrganizationId, PrincipalId},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::{executions::ExecutionId, permissions::PermissionId};
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore};

/// Thirty-two admitted attempts bound retained tasks across the gateway,
/// including lost publishers. Each publisher selects its reviews sequentially.
/// The separate five-second waits bound capacity/acknowledgement observation;
/// they do not cancel physical writes or bound metadata and UI publication.
const MAX_ATTEMPTS: usize = 32;
const ATTEMPT_BUDGET_MS: u64 = 5_000;

/// Immutable evidence of one unpaired `review.notice` policy decision.
/// The system refresh is its initiator; `recipient` is the verified recipient,
/// not a human caller of a permission-selection command.
#[derive(Debug, PartialEq, Eq)]
pub struct ReviewNoticeAuditRecord {
    conversation_id: ConversationId,
    organization_id: OrganizationId,
    owner_id: PrincipalId,
    deleted: bool,
    execution_id: ExecutionId,
    permission_id: PermissionId,
    recipient: AuthContext,
    membership: MembershipRole,
    authority_revision: u64,
    cause: ReviewNoticeAuditCause,
    decision: AccessDecision,
}

/// Why the system is evaluating this recipient's waiting-review selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewNoticeAuditCause {
    /// Initial or refreshed replacement of the recipient's review snapshot.
    SnapshotRefresh,
}

impl ReviewNoticeAuditRecord {
    /// Capture facts after checked metadata and current-session admission, with
    /// the exact policy answer. No timestamp or storage identity is invented here.
    pub(super) fn new(
        conversation: &Conversation,
        execution_id: ExecutionId,
        permission_id: PermissionId,
        recipient: &AuthContext,
        current: &AccessSnapshot,
        cause: ReviewNoticeAuditCause,
        decision: AccessDecision,
    ) -> Self {
        Self {
            conversation_id: conversation.id().clone(),
            organization_id: conversation.organization().clone(),
            owner_id: conversation.owner().clone(),
            deleted: conversation.deletion().is_some(),
            execution_id,
            permission_id,
            recipient: recipient.clone(),
            membership: current.membership.role(),
            authority_revision: current.revision,
            cause,
            decision,
        }
    }

    /// Requested conversation, correlated before policy evaluation.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }
    /// Conversation's organization at selection.
    pub fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
    /// Conversation's owner at selection.
    pub fn owner_id(&self) -> &PrincipalId {
        &self.owner_id
    }
    /// Whether the selected metadata was a tombstone.
    pub fn deleted(&self) -> bool {
        self.deleted
    }
    /// Execution that owns the waiting permission.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }
    /// Waiting permission selected for this recipient.
    pub fn permission_id(&self) -> &PermissionId {
        &self.permission_id
    }
    /// Verified recipient selectors, without any credential secret.
    pub fn recipient(&self) -> &AuthContext {
        &self.recipient
    }
    /// Current membership fact supplied to Cedar.
    pub fn membership(&self) -> MembershipRole {
        self.membership
    }
    /// Committed authority version read immediately before Cedar.
    pub fn authority_revision(&self) -> u64 {
        self.authority_revision
    }
    /// System transition that caused this policy selection.
    pub fn cause(&self) -> ReviewNoticeAuditCause {
        self.cause
    }
    /// Complete allow, deny or unavailable result, including deciding policies.
    pub fn decision(&self) -> &AccessDecision {
        &self.decision
    }
}

/// An audit attempt did not establish durable acknowledgement. This does not
/// prove that a physical write was absent or rolled back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewNoticeAuditFailure {
    /// No adapter, closed admission, failed persistence, or a lost delivery task.
    Unavailable,
    /// An admission/acknowledgement budget or the shared host deadline passed.
    TimedOut,
    /// The sink or delivery clock panicked.
    Panicked,
}

/// Controlled storage for complete unpaired review-policy evidence.
/// A successful return acknowledges delivery within the adapter's documented
/// storage guarantees, independently of UI availability. Dropping the future must
/// retain already-submitted physical work.
pub trait ReviewNoticeAudit: Send + Sync {
    /// Deliver one immutable selection. No retry is inferred from an uncertain
    /// acknowledgement; the supplied adapter documents its durability boundary.
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>>;
}

/// An owned monotonic wait on the review audit clock.
pub type ReviewNoticeSleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Deadline time for review audit admission and acknowledgement.
/// Milliseconds are measured from one clock's origin and never decrease.
pub trait ReviewNoticeClock: Send + Sync {
    /// Current monotonic milliseconds on this clock's timeline.
    fn now(&self) -> u64;
    /// Resolve when this clock reaches the supplied absolute milliseconds.
    fn sleep_until(&self, deadline: u64) -> ReviewNoticeSleep;
}

/// One gateway's bounded admission and independent review-audit attempts.
pub struct ReviewNoticeAuditor {
    audit: Arc<dyn ReviewNoticeAudit>,
    clock: Arc<dyn ReviewNoticeClock>,
    attempts: Arc<Semaphore>,
    pending: Arc<PendingAudits>,
}

impl ReviewNoticeAuditor {
    /// Composition supplies one sink and monotonic deadline clock for all sockets.
    pub fn new(audit: Arc<dyn ReviewNoticeAudit>, clock: Arc<dyn ReviewNoticeClock>) -> Self {
        Self {
            audit,
            clock,
            attempts: Arc::new(Semaphore::new(MAX_ATTEMPTS)),
            pending: Arc::new(PendingAudits::default()),
        }
    }

    /// Acquire capacity before metadata/current-session reads and policy. A caller
    /// lost here has admitted no decision; dropping this future submits no audit.
    pub(crate) async fn admit(
        &self,
    ) -> Result<ReviewNoticeAuditAdmission, ReviewNoticeAuditFailure> {
        let attempt = async {
            let deadline = self.clock.now().saturating_add(ATTEMPT_BUDGET_MS);
            let permit = tokio::select! { biased;
                permit = self.attempts.clone().acquire_owned() => permit.map_err(|_| ReviewNoticeAuditFailure::Unavailable)?,
                () = self.clock.sleep_until(deadline) => return Err(ReviewNoticeAuditFailure::TimedOut),
            };
            let mut state = self
                .pending
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if state.closed {
                return Err(ReviewNoticeAuditFailure::Unavailable);
            }
            state.active += 1;
            Ok(ReviewNoticeAuditAdmission {
                audit: self.audit.clone(),
                clock: self.clock.clone(),
                attempt: PendingAudit {
                    pending: self.pending.clone(),
                    unacknowledged: false,
                    _permit: permit,
                },
            })
        };
        contain_failure(attempt).await
    }
    /// Fence new selections before composition polls any cleanup owner.
    pub(crate) fn close(&self) {
        self.pending
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = true;
        self.attempts.close();
        self.pending.changed.notify_waiters();
    }

    /// Observe all admitted selections, including those still before policy.
    /// Composition closes admission first; only acknowledged decisions and
    /// abandoned pre-decision admissions can establish a successful drain.
    pub(crate) async fn drain(&self) -> Result<(), ReviewNoticeAuditFailure> {
        loop {
            let changed = self.pending.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self
                    .pending
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if state.closed && state.active == 0 {
                    return state.failure.map_or(Ok(()), Err);
                }
            }
            changed.await;
        }
    }

    /// The shared host deadline cannot acknowledge remaining work. Retain the
    /// earliest failure so later task completion cannot fabricate success.
    pub(crate) fn expire(&self) -> ReviewNoticeAuditFailure {
        let mut state = self
            .pending
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *state
            .failure
            .get_or_insert(ReviewNoticeAuditFailure::TimedOut)
    }
}

/// Capacity for a decision that has not yet been made.
pub(crate) struct ReviewNoticeAuditAdmission {
    audit: Arc<dyn ReviewNoticeAudit>,
    clock: Arc<dyn ReviewNoticeClock>,
    attempt: PendingAudit,
}

impl ReviewNoticeAuditAdmission {
    /// Synchronously transfer the admitted evidence before the next await. The
    /// returned future only observes the task; its cancellation cannot abort it.
    pub(crate) fn record(
        mut self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> impl Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send {
        let (send, receive) = oneshot::channel();
        // Mark transfer before spawn: even a task dropped before its first poll
        // must leave unacknowledged evidence in the gateway's drain.
        self.attempt.unacknowledged = true;
        tokio::spawn(async move {
            let attempt = async {
                let deadline = self.clock.now().saturating_add(ATTEMPT_BUDGET_MS);
                tokio::select! { biased;
                    result = self.audit.record(record) => result,
                    () = self.clock.sleep_until(deadline) => Err(ReviewNoticeAuditFailure::TimedOut),
                }
            };
            let result = contain_failure(attempt).await;
            self.attempt.finish(result);
            if let Err(failure) = result {
                tracing::error!(?failure, "review-notice policy audit was not acknowledged");
            }
            // Losing this observer never changes or retries the delivery result.
            let _ = send.send(result);
        });
        async move {
            receive
                .await
                .unwrap_or(Err(ReviewNoticeAuditFailure::Unavailable))
        }
    }
}

#[derive(Default)]
struct PendingAudits {
    state: Mutex<PendingAuditState>,
    changed: Notify,
}

#[derive(Default)]
struct PendingAuditState {
    closed: bool,
    active: usize,
    failure: Option<ReviewNoticeAuditFailure>,
}

// The admission itself owns registration until either pre-decision abandonment
// or transferred delivery completes. A lost transferred task is a failure even
// when no task remains to report one. No policy fact is invented before transfer.
struct PendingAudit {
    pending: Arc<PendingAudits>,
    unacknowledged: bool,
    _permit: OwnedSemaphorePermit,
}
impl PendingAudit {
    fn finish(mut self, result: Result<(), ReviewNoticeAuditFailure>) {
        if let Err(failure) = result {
            self.pending
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .failure
                .get_or_insert(failure);
        }
        self.unacknowledged = false;
    }
}
impl Drop for PendingAudit {
    fn drop(&mut self) {
        {
            let mut state = self
                .pending
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self.unacknowledged {
                state
                    .failure
                    .get_or_insert(ReviewNoticeAuditFailure::Unavailable);
            }
            state.active -= 1;
        }
        self.pending.changed.notify_waiters();
    }
}

async fn contain_failure<T>(
    attempt: impl Future<Output = Result<T, ReviewNoticeAuditFailure>>,
) -> Result<T, ReviewNoticeAuditFailure> {
    match AssertUnwindSafe(attempt).catch_unwind().await {
        Ok(result) => result,
        Err(payload) => {
            std::mem::forget(payload);
            Err(ReviewNoticeAuditFailure::Panicked)
        }
    }
}
