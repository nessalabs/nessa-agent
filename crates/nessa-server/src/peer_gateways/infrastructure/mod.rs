//! Peer records on disk, their audit, the connector enrollments dial with,
//! the owner commands over them, and the poller that reads what each peer
//! granted. Design: `docs/design/auth/peer-gateways.md` ("The dialing side",
//! "Reading a peer").
mod audit;
mod commands;
mod connector;
mod poller;
mod records;
pub use audit::DurablePeerAudit;
pub use commands::{
    EnrollmentEntropy, EnrollmentEntropySource, PeerCommands, PeerError, PeerSync, SyncState,
    AUDIT_DEADLINE, CONNECT, PREEMPT,
};
pub use connector::TcpPeerConnector;
pub use poller::{
    PeerPoller, PollInputs, PollPolicy, POLL_BACKOFF_CAP, POLL_INTERVAL, READ_BUDGET,
};
pub use records::{
    ForgetFailure, PeerEntry, PeerPhase, PeerRecord, PeerRecords, PeerSlot, SlotFound, SlotRefusal,
    SlotSave, SlotTransition,
};
