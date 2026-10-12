//! The rules `nessa env serve` decides by, apart from any I/O:
//! `grants.rs` is what this host's owner grants each gateway leasing from
//! it, and whether a lease is admitted under that, and on what terms.
mod grants;
pub(crate) use grants::{
    admit_agent, admit_command, granted_to, AgentName, Agents, AllAgents, CommandGrant,
    GrantOperation, Peer, PeerGrant,
};
