//! The stores one gateway cut must name. A local outbox is not among them.

/// One durable store the cut joins and reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreKind {
    CanonicalRecords,
    ConversationMetadata,
    ShareGrants,
    Notes,
    DriveToolAudit,
    EnvironmentAudit,
    ConversationAudit,
    PeerPairings,
    EnvironmentPeerPairings,
    WorkerEnrollments,
    EnvironmentGrants,
    RelayConfiguration,
    LeaseRecords,
    ArtifactHolds,
    DeviceCredentials,
    PeerCredentials,
    EnvironmentPeerCredentials,
    WorkerCredentials,
    ReceiverAccess,
    DeletionInventory,
}

impl StoreKind {
    /// The spelling used in the manifest.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalRecords => "canonical-records",
            Self::ConversationMetadata => "conversation-metadata",
            Self::ShareGrants => "share-grants",
            Self::Notes => "notes",
            Self::DriveToolAudit => "drive-tool-audit",
            Self::EnvironmentAudit => "environment-audit",
            Self::ConversationAudit => "conversation-audit",
            Self::PeerPairings => "peer-pairings",
            Self::EnvironmentPeerPairings => "environment-peer-pairings",
            Self::WorkerEnrollments => "worker-enrollments",
            Self::EnvironmentGrants => "environment-grants",
            Self::RelayConfiguration => "relay-configuration",
            Self::LeaseRecords => "lease-records",
            Self::ArtifactHolds => "artifact-holds",
            Self::DeviceCredentials => "device-credentials",
            Self::PeerCredentials => "peer-credentials",
            Self::EnvironmentPeerCredentials => "environment-peer-credentials",
            Self::WorkerCredentials => "worker-credentials",
            Self::ReceiverAccess => "receiver-access",
            Self::DeletionInventory => "deletion-inventory",
        }
    }

    /// The store a manifest token names, if it is one this cut keeps.
    pub fn parse(token: &str) -> Option<Self> {
        WRITER_INVENTORY
            .iter()
            .copied()
            .find(|kind| kind.as_str() == token)
    }
}

/// Every store a cut joins, in the order their leases are read.
///
/// A local outbox is absent on purpose: the device outbox lives on the
/// device, and the environment outbox is emptied when serving starts.
pub const WRITER_INVENTORY: &[StoreKind] = &[
    StoreKind::CanonicalRecords,
    StoreKind::ConversationMetadata,
    StoreKind::ShareGrants,
    StoreKind::Notes,
    StoreKind::DriveToolAudit,
    StoreKind::EnvironmentAudit,
    StoreKind::ConversationAudit,
    StoreKind::PeerPairings,
    StoreKind::EnvironmentPeerPairings,
    StoreKind::WorkerEnrollments,
    StoreKind::EnvironmentGrants,
    StoreKind::RelayConfiguration,
    StoreKind::LeaseRecords,
    StoreKind::ArtifactHolds,
    StoreKind::DeviceCredentials,
    StoreKind::PeerCredentials,
    StoreKind::EnvironmentPeerCredentials,
    StoreKind::WorkerCredentials,
    StoreKind::ReceiverAccess,
    StoreKind::DeletionInventory,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_inventory_omits_local_outboxes() {
        assert!(StoreKind::parse("local-outbox").is_none());
        assert!(StoreKind::parse("local-outboxes").is_none());
        assert!(WRITER_INVENTORY
            .iter()
            .all(|kind| !kind.as_str().contains("outbox")));
        assert_eq!(WRITER_INVENTORY.len(), 20);
        let mut ordered = WRITER_INVENTORY.to_vec();
        ordered.sort();
        ordered.dedup();
        assert_eq!(ordered.len(), WRITER_INVENTORY.len());
        // Exhaustive so a new store cannot compile until the inventory names it.
        for kind in WRITER_INVENTORY {
            match kind {
                StoreKind::CanonicalRecords
                | StoreKind::ConversationMetadata
                | StoreKind::ShareGrants
                | StoreKind::Notes
                | StoreKind::DriveToolAudit
                | StoreKind::EnvironmentAudit
                | StoreKind::ConversationAudit
                | StoreKind::PeerPairings
                | StoreKind::EnvironmentPeerPairings
                | StoreKind::WorkerEnrollments
                | StoreKind::EnvironmentGrants
                | StoreKind::RelayConfiguration
                | StoreKind::LeaseRecords
                | StoreKind::ArtifactHolds
                | StoreKind::DeviceCredentials
                | StoreKind::PeerCredentials
                | StoreKind::EnvironmentPeerCredentials
                | StoreKind::WorkerCredentials
                | StoreKind::ReceiverAccess
                | StoreKind::DeletionInventory => {}
            }
        }
    }
}
