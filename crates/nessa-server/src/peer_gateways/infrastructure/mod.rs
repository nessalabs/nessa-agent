//! Peer records on disk and the owner commands over them. Design:
//! `docs/design/auth/peer-gateways.md` ("The dialing side").
mod commands;
mod records;
pub use commands::{PeerCommands, PeerError};
pub use records::{PeerEntry, PeerPhase, PeerRecord, PeerRecords, PeerSlot, SlotRefusal};
