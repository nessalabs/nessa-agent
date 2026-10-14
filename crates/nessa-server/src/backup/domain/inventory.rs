//! The stores one gateway cut must name, including both local outboxes.

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
    DeviceOutbox,
    EnvironmentOutbox,
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
            Self::DeviceOutbox => "device-outbox",
            Self::EnvironmentOutbox => "environment-outbox",
            Self::DeletionInventory => "deletion-inventory",
        }
    }

    /// The device outbox and the environment outbox. A failure or a skipped
    /// row aborts the cut instead of publishing the other stores without it.
    /// `a_failed_outbox_source_aborts_the_cut` and
    /// `a_skipped_outbox_row_aborts_the_cut`.
    pub const fn requires_every_row(self) -> bool {
        matches!(self, Self::DeviceOutbox | Self::EnvironmentOutbox)
    }

    /// The store a manifest token names, if it is one this cut keeps.
    pub fn parse(token: &str) -> Option<Self> {
        WRITER_INVENTORY
            .iter()
            .copied()
            .find(|kind| kind.as_str() == token)
    }
}

/// Every store a cut joins, in the order their snapshots are taken.
///
/// The device outbox and the environment outbox are pinned with the other
/// stores, before deletion fences are appended.
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
    StoreKind::DeviceOutbox,
    StoreKind::EnvironmentOutbox,
    StoreKind::DeletionInventory,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_inventory_names_both_outboxes() {
        assert_eq!(
            StoreKind::parse("device-outbox"),
            Some(StoreKind::DeviceOutbox)
        );
        assert_eq!(
            StoreKind::parse("environment-outbox"),
            Some(StoreKind::EnvironmentOutbox)
        );
        assert!(StoreKind::DeviceOutbox.requires_every_row());
        assert!(StoreKind::EnvironmentOutbox.requires_every_row());
        assert!(WRITER_INVENTORY
            .iter()
            .filter(|kind| !kind.requires_every_row())
            .all(|kind| !kind.as_str().contains("outbox")));
        assert_eq!(WRITER_INVENTORY.len(), 22);
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
                | StoreKind::DeviceOutbox
                | StoreKind::EnvironmentOutbox
                | StoreKind::DeletionInventory => {}
            }
        }
    }
}
