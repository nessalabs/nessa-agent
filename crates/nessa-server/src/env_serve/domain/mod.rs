//! The rules `nessa env serve` decides by, apart from any I/O:
//! `grants.rs` is what this host's owner grants each gateway leasing from
//! it, and the terms a lease the lease policy allows is admitted on.
mod grants;
pub(crate) use grants::{
    command_timeout, lease_ms, unwritten, AgentName, Agents, AllAgents, CommandGrant,
    GrantOperation, MaxLease, NotGranted, Peer, PeerGrant,
};
