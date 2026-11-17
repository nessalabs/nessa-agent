//! Where each agent's saved login is kept.
use std::{collections::BTreeMap, sync::Arc};

use nessa_protocol::agents::AgentId;
use nessa_vault::{application::Vault, domain::EntryName};

use crate::logins::domain::login_entry;
use crate::secrets::{MovedSecret, SecretStore, VaultSecret};

/// Where each agent's saved login is kept: its one vault entry. Build these
/// once and share them. Claude's is also the sign-in Boat VMs are served
/// with, which an earlier build kept in its own `boat.claude-sign-in` entry;
/// that one is moved here when first read, so one entry holds the fact.
pub fn login_stores(vault: &Arc<Vault>) -> BTreeMap<AgentId, Arc<dyn SecretStore>> {
    AgentId::ALL
        .iter()
        .map(|agent| (*agent, login_store(vault, *agent)))
        .collect()
}

fn login_store(vault: &Arc<Vault>, agent: AgentId) -> Arc<dyn SecretStore> {
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
