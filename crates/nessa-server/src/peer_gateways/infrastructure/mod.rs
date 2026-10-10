//! Peer records on disk, their audit, the connector enrollments dial with,
//! and the owner commands over them.
//! Design: `docs/design/auth/peer-gateways.md` ("The dialing side").
mod audit;
mod commands;
mod connector;
mod records;
pub use audit::DurablePeerAudit;
pub use commands::{PeerCommands, PeerError, AUDIT_DEADLINE, CONNECT};
pub use connector::TcpPeerConnector;
pub use records::{
    PeerEntry, PeerPhase, PeerRecord, PeerRecords, PeerSlot, SlotFound, SlotRefusal, SlotSave,
};
