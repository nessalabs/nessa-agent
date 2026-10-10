//! Peer records on disk, their audit, the owner commands over them, and the
//! poller that reads what each peer granted. Design:
//! `docs/design/auth/peer-gateways.md` ("The dialing side", "Reading a peer").
mod audit;
mod commands;
mod poller;
mod records;
pub use audit::DurablePeerAudit;
pub use commands::{PeerCommands, PeerError, PeerSync, SyncState};
pub use poller::{PeerPoller, PollPolicy, POLL_BACKOFF_CAP, POLL_INTERVAL};
pub use records::{
    PeerEntry, PeerPhase, PeerRecord, PeerRecords, PeerSlot, SlotRefusal, SlotSave,
};
