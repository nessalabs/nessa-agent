//! Durable peer command evidence: one private file per record, named by the
//! record's own id, published only if that name is unused, then synced with
//! its directory before the record is acknowledged. Separate files cannot
//! tear each other, and a record is never replaced.
//!
//! The record id and `observedAtMs` are assigned here: the id fresh, the time
//! from the injected wall clock, when this adapter took the record. That is
//! not claimed as the time of the peer's own effect. One write runs at a time.
//! A process that stops between a command's effect and its outcome record
//! leaves the intent alone: the intent says what was asked, and the peer
//! record on disk says what it came to.
use crate::peer_gateways::application::{
    CacheState, PeerAudit, PeerAuditFuture, PeerAuditRecord, PeerAuditUnavailable, PeerHolding,
    PeerState, PollerCause,
};
use nessa_auth::{
    application::ports::Clock,
    domain::{pairing::DeviceKey, PrincipalId},
};
use nessa_local_storage::{sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// Peer command evidence in one private directory.
pub struct DurablePeerAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
    /// One blocking write at a time, held by that write until it returns.
    write: Arc<Semaphore>,
}
impl DurablePeerAudit {
    /// Records go in `directory`, which composition has already created
    /// private; an unsafe or missing one fails each write, never repaired.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self {
            directory,
            clock,
            write: Arc::new(Semaphore::new(1)),
        }
    }
}
impl PeerAudit for DurablePeerAudit {
    fn record(&self, record: PeerAuditRecord) -> PeerAuditFuture<'_> {
        let id = Uuid::new_v4().to_string();
        let mut value = record_value(&record);
        value["recordId"] = json!(id);
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        let directory = self.directory.clone();
        let write = self.write.clone();
        Box::pin(async move {
            let permit = write
                .acquire_owned()
                .await
                .map_err(|_| PeerAuditUnavailable)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut file = PrivateTempFile::new_in(&directory)?;
                serde_json::to_writer(file.as_file_mut(), &value)?;
                file.as_file_mut().write_all(b"\n")?;
                file.as_file().sync_all()?;
                file.publish(&directory.join(format!("{id}.json")))?;
                sync_directory(&directory)
            })
            .await
            .map_err(|_| PeerAuditUnavailable)?
            .map_err(|error| {
                tracing::error!(%error, "peer gateway audit record was not committed");
                PeerAuditUnavailable
            })
        })
    }
}

fn hex(key: &DeviceKey) -> String {
    key.bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn person(initiator: &PrincipalId) -> Value {
    json!({"kind": "principal", "principalId": initiator.as_str()})
}

fn state(of: &PeerState) -> Value {
    phase(of)
}

fn phase(state: &PeerState) -> Value {
    match state {
        PeerState::Absent => json!({"phase": "absent"}),
        PeerState::Pending { address } => {
            json!({"phase": "pending", "address": address.to_string()})
        }
        PeerState::Active {
            address,
            credential,
            receiver,
        } => json!({"phase": "active", "address": address.to_string(),
            "credentialId": credential, "receiverId": receiver}),
        PeerState::Revoked { address } => {
            json!({"phase": "revoked", "address": address.to_string()})
        }
        PeerState::Unreadable => json!({"phase": "unreadable"}),
        PeerState::Unknown => json!({"phase": "unknown"}),
        PeerState::NotRead => json!({"phase": "not_read"}),
        PeerState::CacheUnconfirmed { record } => {
            let mut value = phase(record);
            value["cache"] = json!("unconfirmed");
            value
        }
    }
}

fn holding(holding: &PeerHolding) -> Value {
    let mut value = state(&holding.record);
    value["cache"] = json!(match holding.cache {
        CacheState::Present => "present",
        CacheState::Absent => "absent",
        CacheState::Unknown => "unknown",
    });
    value
}

/// The cause's name, and what it names: the status's own cause or outcome,
/// or the conversation withdrawn.
fn cause(cause: &PollerCause) -> (&'static str, Option<&str>) {
    match cause {
        PollerCause::Approved => ("peer_approved", None),
        PollerCause::Ended { detail } => ("peer_ended", detail.as_deref()),
        PollerCause::ResetRequired => ("cache_reset_required", None),
        PollerCause::CacheDamaged => ("cache_damaged", None),
        PollerCause::Withdrawn { .. } => ("peer_withdrew", None),
    }
}

fn outcome(outcome: &Result<(), &'static str>) -> Value {
    match outcome {
        Ok(()) => json!({"result": "succeeded"}),
        Err(code) => json!({"result": "refused", "code": code}),
    }
}

/// Everything except the record's id and observation time.
pub(super) fn record_value(record: &PeerAuditRecord) -> Value {
    match record {
        PeerAuditRecord::EnrollRequested {
            operation,
            initiator,
            address,
        } => json!({
            "kind": "peer_enroll_requested",
            "operationId": operation.to_string(),
            "target": {"address": address.to_string()},
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::EnrollFinished {
            operation,
            initiator,
            address,
            peer,
            before,
            after,
            outcome: result,
        } => json!({
            "kind": "peer_enroll_finished",
            "operationId": operation.to_string(),
            "target": {"address": address.to_string(), "peerKey": peer.as_ref().map(hex)},
            "transition": {"before": state(before), "after": state(after)},
            "outcome": outcome(result),
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::ForgetRequested {
            operation,
            initiator,
            peer,
        } => json!({
            "kind": "peer_forget_requested",
            "operationId": operation.to_string(),
            "target": {"peerKey": hex(peer)},
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::ForgetFinished {
            operation,
            initiator,
            peer,
            before,
            after,
            outcome: result,
        } => json!({
            "kind": "peer_forget_finished",
            "operationId": operation.to_string(),
            "target": {"peerKey": hex(peer)},
            "transition": {"before": state(before), "after": state(after)},
            "outcome": outcome(result),
            "cause": "owner_requested",
            "initiator": person(initiator),
        }),
        PeerAuditRecord::PollerChanged {
            operation,
            peer,
            cause: why,
            before,
            after,
            outcome: result,
        } => {
            let (name, detail) = cause(why);
            let conversation = match why {
                PollerCause::Withdrawn { conversation } => Some(conversation.as_str()),
                _ => None,
            };
            json!({
                "kind": "peer_poller_changed",
                "operationId": operation.to_string(),
                "target": {"peerKey": hex(peer), "conversationId": conversation},
                "transition": {"before": holding(before), "after": holding(after)},
                "outcome": outcome(result),
                "cause": name,
                "causeDetail": detail,
                "initiator": {"kind": "system", "component": "peer_poller"},
            })
        }
    }
}
