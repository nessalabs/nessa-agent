//! Immutable ownership transition evidence using the existing durable audit writer.
use super::mode_audit::write_once;
use crate::conversation::application::ConversationError;
use async_trait::async_trait;
use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::subagents::{OwnershipAudit, PortFailure};
use nessa_sdk::domain::agent_execution::subagents::{Initiator, OwnershipEvidence};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

pub struct DurableOwnershipAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl DurableOwnershipAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        nessa_local_storage::create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}
#[async_trait]
impl OwnershipAudit for DurableOwnershipAudit {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        let initiator = match &evidence.initiator {
            Initiator::Host(actor) => {
                json!({"kind":"host", "principalId":actor.principal_id(), "surfaceId":actor.surface_id(), "requestId":actor.request_id()})
            }
            Initiator::Runtime => json!({"kind":"runtime"}),
        };
        let mut value = json!({"kind":"owned_thread_transition", "parentLifetime":evidence.parent_lifetime.as_str(),
            "childLifetime":evidence.child_lifetime.as_ref().map(|id| id.as_str()),
            "closeOperation":evidence.close_operation.as_ref().map(|id| id.as_str()),
            "before":format!("{:?}", evidence.before), "after":format!("{:?}", evidence.after),
            "cause":evidence.cause.as_ref().map(|cause| format!("{cause:?}")),
            "closeDetail":evidence.close_detail.as_ref().map(|detail| format!("{detail:?}")), "initiator":initiator});
        let bytes = serde_json::to_vec(&value).map_err(|_| PortFailure::Rejected)?;
        let id = format!("ownership-{:x}", Sha256::digest(bytes));
        value["observedAtMs"] = json!(self.clock.unix_milliseconds());
        // A failed physical acknowledgement is uncertain; do not promise rollback.
        write_once(self.directory.clone(), id, value)
            .await
            .map_err(|_| PortFailure::Uncertain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::domain::agent_execution::subagents::{AgentLifetimeId, OwnershipMeaning};
    use serde_json::Value;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestClock(AtomicU64);
    impl Clock for TestClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    fn evidence() -> OwnershipEvidence {
        OwnershipEvidence {
            parent_lifetime: AgentLifetimeId::new("parent").unwrap(),
            child_lifetime: None,
            close_operation: None,
            close_detail: None,
            before: OwnershipMeaning::Absent,
            after: OwnershipMeaning::Open,
            cause: None,
            initiator: Initiator::Runtime,
        }
    }
    #[tokio::test]
    async fn ownership_audit_retry_retains_first_observation_after_clock_and_adapter_restart() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("ownership");
        let clock = Arc::new(TestClock(42.into()));
        let audit = DurableOwnershipAudit::new(directory.clone(), clock.clone()).unwrap();
        audit.record(&evidence()).await.unwrap();
        let files: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|file| file.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let first = std::fs::read(&files[0]).unwrap();
        let value: Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(value["observedAtMs"], 42);
        assert_eq!(value["parentLifetime"], "parent");
        assert_eq!(value["before"], "Absent");
        assert_eq!(value["after"], "Open");
        assert_eq!(value["initiator"]["kind"], "runtime");
        clock.0.store(900, Ordering::SeqCst);
        drop(audit);
        let restarted = DurableOwnershipAudit::new(directory.clone(), clock).unwrap();
        restarted.record(&evidence()).await.unwrap();
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(std::fs::read(&files[0]).unwrap(), first);
        std::fs::remove_file(&files[0]).unwrap();
        std::fs::remove_dir(&directory).unwrap();
        std::fs::write(&directory, b"not a directory").unwrap();
        assert_eq!(
            restarted.record(&evidence()).await,
            Err(PortFailure::Uncertain)
        );
    }
}
