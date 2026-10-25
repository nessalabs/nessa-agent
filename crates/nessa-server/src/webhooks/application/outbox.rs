//! Durable outbox operations acknowledge complete state and audit commits.
use super::{MacKey, Target};
use crate::webhooks::domain::{Delivery, Destination, Observation, Phase};
use async_trait::async_trait;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

pub const PENDING_LIMIT: usize = 1024;
pub const MAX_REGISTRATIONS: usize = 64;
pub const MAX_OUTBOX_WORKERS: usize = 4;
pub const RETAINED_LIMIT: usize = 1024;
pub const MAX_CURSORS: usize = 1024;
pub const MAX_SOURCE_RESETS: usize = MAX_CURSORS + PENDING_LIMIT + RETAINED_LIMIT;
pub const SOURCE_RESET_BYTES: usize = 16 * 1024;

/// One committed registration. Debug deliberately excludes endpoint and key.
#[derive(Clone)]
pub struct Registration {
    pub id: Uuid,
    pub target: Target,
    pub destination: Destination,
    pub key: Arc<MacKey>,
}

/// Exact source bytes and scope selected under current read authority.
pub struct QueuedRecord {
    pub registration: Uuid,
    pub conversation: ConversationId,
    pub scope: Scope,
    pub expected_after: u64,
    pub position: u64,
    pub epoch: u64,
    pub body: Vec<u8>,
    pub id: Uuid,
    pub observed_at_ms: u64,
    pub initiator: QueueInitiator,
}

pub struct PendingDelivery {
    pub registration: Registration,
    pub conversation: ConversationId,
    pub scope: Scope,
    pub position: u64,
    pub delivery: Delivery,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxError {
    Unavailable,
    Invalid,
    Conflict,
    Capacity,
    ResetRequired,
}

/// Identifies the observing source reader independently of affected deliveries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueInitiator {
    Live,
    Restart,
}
impl QueueInitiator {
    pub(crate) fn label(self, registration: Uuid) -> String {
        match self {
            Self::Live => format!("reader:{registration}"),
            Self::Restart => "restart".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryCause {
    Admission,
    AdmissionExpired,
    AuditUnavailable,
    Reconcile,
    Response,
    Deadline,
    AuthorityStopped,
    Refused,
    Restart,
    SourceChanged { initiator: QueueInitiator },
    SourceQueued { initiator: QueueInitiator },
}

impl DeliveryCause {
    /// Persistence checks the proposed replacement against the domain owner.
    /// It does not implement a second transition table or admit authority.
    pub(crate) fn allows(self, before: &Delivery, after: &Delivery, at_ms: u64) -> bool {
        let is_after = |candidate: Result<Delivery, _>| candidate.as_ref() == Ok(after);
        match self {
            Self::Admission => is_after(before.admit(before.epoch(), at_ms)),
            Self::AdmissionExpired => is_after(before.not_dispatched()),
            Self::AuditUnavailable => is_after(before.refused()),
            Self::Reconcile => before.phase() == Phase::Sending && before.recovered() == *after,
            Self::Response => [
                Observation::Success,
                Observation::Retryable,
                Observation::Rejected,
                Observation::TimedOut,
                Observation::Unknown,
            ]
            .into_iter()
            .any(|observation| is_after(before.observed(observation, at_ms))),
            Self::Deadline => {
                is_after(before.observed(Observation::TimedOut, at_ms))
                    || is_after(before.refused())
            }
            Self::AuthorityStopped | Self::Refused | Self::SourceChanged { .. } => {
                is_after(before.refused())
            }
            Self::SourceQueued { .. } => false,
            Self::Restart => before.phase() == Phase::Sending && before.recovered() == *after,
        }
    }
}

#[async_trait]
pub trait Outbox: Send + Sync {
    /// Cursor and the first queued row commit together. Duplicate checkpoints
    /// resolve the original identity; cursor CAS prevents skipping a predecessor.
    async fn queue(&self, record: QueuedRecord) -> Result<Uuid, OutboxError>;
    /// The sole sender reads bounded work from durable state, not an in-memory
    /// notification queue. Sending rows are recovered before startup admission.
    async fn due(&self, now_ms: u64) -> Result<Vec<PendingDelivery>, OutboxError>;
    /// Compare-and-swap the complete prior value and append immutable transition
    /// evidence in one commit. Acknowledgement precedes any POST effect.
    async fn transition(
        &self,
        before: &Delivery,
        after: &Delivery,
        cause: DeliveryCause,
        at_ms: u64,
    ) -> Result<(), OutboxError>;
    /// Startup only, before the sole sender begins. Sending has no known remote
    /// observation and becomes terminal Unknown; it is never automatically POSTed.
    async fn recover(&self, at_ms: u64) -> Result<(), OutboxError>;
}
