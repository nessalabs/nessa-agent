//! The deletion inventory written after a fence commits, and when an
//! operator asks. Neither path takes conversation admission or publishes
//! a bundle.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use nessa_auth::application::ports::Clock;

use super::cedar::CedarInventoryPolicy;
use super::decision_audit::FileInventoryDecisions;
use super::gateway::read_committed_fences;
use super::log::FileDeletionLog;
use crate::backup::application::{
    capture_inventory, CaptureError, FenceSource, InventoryAction, InventoryCause, InventoryFacts,
};
use crate::backup::domain::{DeletionFence, DeletionLog};
use crate::conversation::application::{FenceCapture, FenceCaptureError};

/// The gateway's deletion inventory: the log beside `namespace/backup` and
/// the Cedar policy that decides who may write it.
pub struct CommittedInventory {
    namespace: PathBuf,
    log: FileDeletionLog,
    policy: CedarInventoryPolicy,
    decisions: FileInventoryDecisions,
}

impl CommittedInventory {
    /// Open or create `namespace/backup`. The mac key is created when absent.
    /// `clock` is the observation time of each policy decision.
    ///
    /// # Errors
    /// The directory, mac key, or decision log could not be opened, or the
    /// inventory policy does not validate. A decision log that is not a
    /// sequence of `inventory-decision-N` lines is refused.
    pub fn open(namespace: impl AsRef<Path>, clock: Arc<dyn Clock>) -> Result<Self, String> {
        let namespace = namespace.as_ref().to_path_buf();
        let backup = namespace.join("backup");
        let log = FileDeletionLog::open(&backup)
            .map_err(|error| format!("the deletion inventory could not be opened: {error}"))?;
        let decisions = FileInventoryDecisions::open(&backup, clock)
            .map_err(|error| format!("the inventory decisions could not be opened: {error}"))?;
        let policy = CedarInventoryPolicy::new()?;
        Ok(Self {
            namespace,
            log,
            policy,
            decisions,
        })
    }

    /// Capture because `kind` asked. A person or the gateway's own surface
    /// may. A device, peer, environment peer, or worker is refused before
    /// the log is read.
    pub async fn capture_as(&self, kind: &str) -> Result<DeletionLog, CaptureError> {
        self.write(&InventoryFacts {
            kind,
            cause: InventoryCause::Operator,
            action: InventoryAction::Capture,
        })
        .await
    }

    async fn write(&self, facts: &InventoryFacts<'_>) -> Result<DeletionLog, CaptureError> {
        let fences = CommittedFences {
            namespace: &self.namespace,
        };
        capture_inventory(facts, &self.policy, &self.decisions, &[&fences], &self.log).await
    }
}

struct CommittedFences<'a> {
    namespace: &'a Path,
}

#[async_trait]
impl FenceSource for CommittedFences<'_> {
    async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
        read_committed_fences(self.namespace).await
    }
}

#[async_trait]
impl FenceCapture for CommittedInventory {
    async fn capture(&self) -> Result<(), FenceCaptureError> {
        self.write(&InventoryFacts {
            kind: "gateway",
            cause: InventoryCause::CommittedFence,
            action: InventoryAction::Record,
        })
        .await
        .map(|_| ())
        .map_err(|error| match error {
            CaptureError::Denied(_) => FenceCaptureError::Refused,
            CaptureError::NotAudited | CaptureError::Inventory(_) => FenceCaptureError::NotStored,
        })
    }
}
