//! A review selection is acknowledged before it may enter a socket's UI queue.
//!
//! Admission bounds owned attempts across sockets. After Cedar answers, `record`
//! transfers the complete selection to an independent task synchronously. A
//! publisher may disappear without cancelling that attempt. Its observer has a
//! deadline; physical persistence remains the adapter's responsibility.
use super::AccessDecision;
use crate::conversation::domain::Conversation;
use futures_util::FutureExt;
use nessa_auth::{
    application::ports::AccessSnapshot,
    domain::{AuthContext, MembershipRole, OrganizationId, PrincipalId},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::{executions::ExecutionId, permissions::PermissionId};
use std::{future::Future, panic::AssertUnwindSafe, pin::Pin, sync::Arc};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

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
    /// No adapter, closed admission, or failed persistence.
    Unavailable,
    /// The injected clock's delivery budget passed.
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
}

impl ReviewNoticeAuditor {
    /// Composition supplies one sink and monotonic deadline clock for all sockets.
    pub fn new(audit: Arc<dyn ReviewNoticeAudit>, clock: Arc<dyn ReviewNoticeClock>) -> Self {
        Self {
            audit,
            clock,
            attempts: Arc::new(Semaphore::new(MAX_ATTEMPTS)),
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
            Ok(ReviewNoticeAuditAdmission {
                audit: self.audit.clone(),
                clock: self.clock.clone(),
                permit,
            })
        };
        contain_failure(attempt).await
    }
}

/// Capacity for a decision that has not yet been made.
pub(crate) struct ReviewNoticeAuditAdmission {
    audit: Arc<dyn ReviewNoticeAudit>,
    clock: Arc<dyn ReviewNoticeClock>,
    permit: OwnedSemaphorePermit,
}

impl ReviewNoticeAuditAdmission {
    /// Synchronously transfer the admitted evidence before the next await. The
    /// returned future only observes the task; its cancellation cannot abort it.
    pub(crate) fn record(
        self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> impl Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send {
        let (send, receive) = oneshot::channel();
        tokio::spawn(async move {
            let _permit = self.permit;
            let attempt = async {
                let deadline = self.clock.now().saturating_add(ATTEMPT_BUDGET_MS);
                tokio::select! { biased;
                    result = self.audit.record(record) => result,
                    () = self.clock.sleep_until(deadline) => Err(ReviewNoticeAuditFailure::TimedOut),
                }
            };
            let result = contain_failure(attempt).await;
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
