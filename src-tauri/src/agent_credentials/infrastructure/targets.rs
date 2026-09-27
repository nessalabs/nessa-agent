use std::sync::LazyLock;

use nessa_agent_credentials::{CredentialAgent, CredentialNamespace};
use serde::Deserialize;

use crate::agent_credentials::{
    application::{CredentialSaveTargets, CredentialStoreFailure},
    domain::value_objects::CredentialSaveTarget,
};

const CREDENTIALS_JSON: &str = include_str!("../../../../protocol/defaults/agent-credentials.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialItems {
    service: String,
    accounts: CredentialAccounts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialAccounts {
    claude: String,
    opencode: String,
}

static ITEMS: LazyLock<CredentialItems> = LazyLock::new(|| {
    serde_json::from_str(CREDENTIALS_JSON)
        .expect("bundled agent-credentials.json must describe keychain items")
});

/// Pure canonical target mapping selected independently of the keychain writer.
pub struct CanonicalCredentialSaveTargets {
    namespace: CredentialNamespace,
}

impl CanonicalCredentialSaveTargets {
    pub fn new(namespace: CredentialNamespace) -> Self {
        Self { namespace }
    }
}

impl CredentialSaveTargets for CanonicalCredentialSaveTargets {
    fn target(
        &self,
        agent: CredentialAgent,
    ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
        let item = match agent {
            CredentialAgent::Claude => &ITEMS.accounts.claude,
            CredentialAgent::Opencode => &ITEMS.accounts.opencode,
        };
        let account = self
            .namespace
            .account(item)
            .map_err(|_| CredentialStoreFailure::Invalid)?;
        CredentialSaveTarget::new(
            agent,
            self.namespace.clone(),
            ITEMS.service.clone(),
            account,
        )
        .map_err(|_| CredentialStoreFailure::Invalid)
    }
}
