//! Private immutable review-policy selections, acknowledged after file sync and
//! create-only publication, with directory sync where the platform supports it.
//! Windows provides no directory-entry or arbitrary power-loss durability here.
//! Each adapter has one shared physical worker: loss of an async observer leaves
//! a submitted write and its admission with that worker.
//!
//! Composition supplies the directory, writer namespace and observation clock.
//! A failed/uncertain write is not retried or removed. Records describe audited
//! selection, not actual socket delivery. Runtime shutdown and process crashes
//! may interrupt unacknowledged work; there is no replay claim for it.
use crate::conversation::application::{
    AccessDecision, AccessDenial, ReviewNoticeAudit, ReviewNoticeAuditCause,
    ReviewNoticeAuditFailure, ReviewNoticeAuditRecord, ReviewNoticeClock, ReviewNoticeSleep,
};
use nessa_auth::{application::ports::Clock, domain::MembershipRole};
use nessa_local_storage::{
    create_directory, physical_operation::Worker, sync_directory, PrivateTempFile,
};
use nessa_sdk::infrastructure::clock::{Clock as RuntimeClockPort, ClockInstant, RuntimeClock};
use serde_json::{json, Value};
use std::{
    future::Future,
    io::Write,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use uuid::Uuid;

/// Runtime adapter for the application-owned review deadline clock.
#[derive(Default)]
pub struct MonotonicReviewNoticeClock(RuntimeClock);

impl MonotonicReviewNoticeClock {
    /// Start one monotonic timeline for the gateway's audit attempts.
    pub fn new() -> Self {
        Self(RuntimeClock::new())
    }
}

impl ReviewNoticeClock for MonotonicReviewNoticeClock {
    fn now(&self) -> u64 {
        u64::try_from(self.0.now().since_origin().as_millis()).unwrap_or(u64::MAX)
    }
    fn sleep_until(&self, deadline: u64) -> ReviewNoticeSleep {
        self.0
            .sleep_until(ClockInstant::from_origin(Duration::from_millis(deadline)))
    }
}

/// Durable local storage independent of the response/UI queue.
pub struct DurableReviewNoticeAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
    writer_namespace: Uuid,
    sequence: AtomicU64,
    worker: Worker,
}

impl DurableReviewNoticeAudit {
    /// Prepare private storage. The injected namespace belongs to this writer
    /// lifetime; sequence allocation refuses exhaustion instead of overwriting.
    pub fn new(
        directory: PathBuf,
        clock: Arc<dyn Clock>,
        writer_namespace: Uuid,
    ) -> Result<Self, ReviewNoticeAuditFailure> {
        create_directory(&directory).map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
        Ok(Self {
            directory,
            clock,
            writer_namespace,
            sequence: AtomicU64::new(0),
            worker: Worker::new(),
        })
    }
}

impl ReviewNoticeAudit for DurableReviewNoticeAudit {
    fn record(
        &self,
        record: Arc<ReviewNoticeAuditRecord>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ReviewNoticeAuditFailure>> + Send + '_>> {
        Box::pin(async move {
            let permit = self
                .worker
                .admit()
                .await
                .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
            let sequence = self
                .sequence
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |next| {
                    next.checked_add(1)
                })
                .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
            let id = format!("review-notice-{}-{sequence}", self.writer_namespace);
            let directory = self.directory.clone();
            let clock = self.clock.clone();
            permit
                .submit(move || {
                    let value = record_value(&id, &record, clock.unix_milliseconds());
                    let mut file = PrivateTempFile::new_in(&directory)
                        .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
                    serde_json::to_writer(file.as_file_mut(), &value)
                        .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
                    file.as_file_mut()
                        .write_all(b"\n")
                        .and_then(|()| file.as_file().sync_all())
                        .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
                    file.publish(&directory.join(format!("{id}.json")))
                        .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?;
                    sync_directory(&directory).map_err(|_| ReviewNoticeAuditFailure::Unavailable)
                })
                .await
                .map_err(|_| ReviewNoticeAuditFailure::Unavailable)?
        })
    }
}

fn record_value(id: &str, record: &ReviewNoticeAuditRecord, observed_at_ms: u64) -> Value {
    let decision = match record.decision() {
        AccessDecision::Allow { policies } => json!({ "kind": "allow", "policies": policies }),
        AccessDecision::Deny(AccessDenial::Forbidden { policies }) => {
            json!({ "kind": "deny", "reason": "forbidden", "policies": policies })
        }
        AccessDecision::Deny(AccessDenial::NotPermitted) => {
            json!({ "kind": "deny", "reason": "not_permitted", "policies": [] })
        }
        AccessDecision::Deny(AccessDenial::Unavailable) => {
            json!({ "kind": "unavailable", "policies": [] })
        }
    };
    let membership = match record.membership() {
        MembershipRole::Admin => "admin",
        MembershipRole::Member => "member",
    };
    let cause = match record.cause() {
        ReviewNoticeAuditCause::SnapshotRefresh => "snapshot_refresh",
    };
    let recipient = record.recipient();
    json!({
        "recordId": id,
        "kind": "review_notice_selection",
        "action": "review.notice",
        "target": {
            "conversationId": record.conversation_id().to_string(),
            "organizationId": record.organization_id().as_str(),
            "ownerId": record.owner_id().as_str(),
            "deleted": record.deleted(),
            "executionId": record.execution_id().as_str(),
            "permissionId": record.permission_id().as_str(),
        },
        "recipient": {
            "organizationId": recipient.organization_id().as_str(),
            "principalId": recipient.principal_id().as_str(),
            "membershipId": recipient.membership_id().as_str(),
            "credentialId": recipient.credential_id().as_str(),
            "audienceId": recipient.audience_id().as_str(),
            "membershipRole": membership,
            "authorityRevision": record.authority_revision(),
        },
        "initiator": { "kind": "system", "cause": cause },
        "decision": decision,
        "observedAtMs": observed_at_ms,
    })
}
