//! Peer records on disk, their audit, and the owner commands over them.
//! Design: `docs/design/auth/peer-gateways.md` ("The dialing side").
mod audit;
mod commands;
mod records;
pub use audit::DurablePeerAudit;
pub use commands::{PeerCommands, PeerError};
pub use records::{PeerEntry, PeerPhase, PeerRecord, PeerRecords, PeerSlot, SlotRefusal, SlotSave};
