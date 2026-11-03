//! Where each agent's saved login is kept.
use std::sync::Arc;

use nessa_protocol::agents::AgentId;
use nessa_vault::{application::Vault, domain::EntryName};

use crate::logins::domain::login_entry;
use crate::secrets::{MovedSecret, SecretStore, VaultSecret};

/// Where `agent`'s saved login is kept: its one vault entry. Claude's is
/// also the sign-in Boat VMs are served with, which an earlier build kept in
/// its own `boat.claude-sign-in` entry; that one is moved here when first
/// read, so one entry holds the fact.
pub fn login_store(vault: &Arc<Vault>, agent: AgentId) -> Arc<dyn SecretStore> {
    let now = Arc::new(VaultSecret::new(vault.clone(), login_entry(agent)));
    match agent {
        AgentId::Claude => Arc::new(MovedSecret::new(
            now,
            Arc::new(VaultSecret::new(
                vault.clone(),
                EntryName::new("boat.claude-sign-in").expect("static vault entry"),
            )),
        )),
        AgentId::Codex | AgentId::Opencode => now,
    }
}
