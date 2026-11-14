//! Peer records on disk, their audit, the connector enrollments dial with,
//! the owner commands over them, the poller that reads what each peer
//! granted, and the local-network announcement of a paired gateway.
//! Design: `docs/design/auth/peer-gateways.md` ("The dialing side",
//! "Reading a peer", "Local discovery").
mod audit;
mod commands;
mod connector;
mod lan;
mod poller;
mod records;
pub use audit::{DurablePeerAudit, AUDIT_HEADROOM, AUDIT_QUEUE};
pub use commands::{
    EnrollmentEntropy, EnrollmentEntropySource, PeerCommands, PeerError, PeerSync, SyncState,
    AUDIT_DEADLINE, CONNECT, PREEMPT,
};
pub use connector::TcpPeerConnector;
pub use lan::{
    payload_from_registration, platform_lan_announcer, registration_properties, LanAnnouncer,
    LanRegistration, PEER_SERVICE,
};
pub use poller::{
    PeerPoller, PollInputs, PollPolicy, POLL_BACKOFF_CAP, POLL_INTERVAL, READS_AT_ONCE,
    READ_BUDGET, WITHDRAWN_PER_READ,
};
pub use records::{
    DiscoveryAuthoritySaveError, ForgetFailure, PeerEntry, PeerPhase, PeerRecord, PeerRecords,
    PeerSlot, SlotFound, SlotOutcome, SlotRefusal, SlotSave, SlotTransition,
};
