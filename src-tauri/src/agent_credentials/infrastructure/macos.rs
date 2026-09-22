use std::sync::LazyLock;

use nessa_agent_credentials::{
    AgentCredential, AgentCredentialKind, CredentialAgent, CredentialNamespace,
};
use security_framework::passwords::set_generic_password;
use serde::Deserialize;

use crate::agent_credentials::application::{AgentCredentialStore, CredentialStoreFailure};

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

/// The macOS login-keychain writer selected by desktop composition.
pub struct LocalAgentCredentialStore {
    namespace: CredentialNamespace,
}

impl LocalAgentCredentialStore {
    /// Store credentials under the exact durable service namespace.
    pub fn new(namespace: CredentialNamespace) -> Self {
        Self { namespace }
    }

    fn account(&self, agent: CredentialAgent) -> Result<String, CredentialStoreFailure> {
        let item = match agent {
            CredentialAgent::Claude => &ITEMS.accounts.claude,
            CredentialAgent::Opencode => &ITEMS.accounts.opencode,
        };
        self.namespace
            .account(item)
            .map_err(|_| CredentialStoreFailure::Invalid)
    }
}

impl AgentCredentialStore for LocalAgentCredentialStore {
    fn save_api_key(
        &self,
        agent: CredentialAgent,
        credential: &AgentCredential,
    ) -> Result<(), CredentialStoreFailure> {
        if credential.kind() != AgentCredentialKind::ApiKey {
            return Err(CredentialStoreFailure::Invalid);
        }
        let account = self.account(agent)?;
        set_generic_password(&ITEMS.service, &account, credential.expose().as_bytes())
            .map_err(|_| CredentialStoreFailure::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use security_framework::passwords::{delete_generic_password, get_generic_password};
    use std::{process, time::SystemTime};

    #[test]
    fn account_names_use_the_shared_durable_namespace() {
        let store = LocalAgentCredentialStore::new(
            CredentialNamespace::new("ci".into(), Some("one".into())).unwrap(),
        );
        assert_eq!(
            store.account(CredentialAgent::Claude).unwrap(),
            "ci:one:claude-api-key"
        );
        assert_eq!(
            store.account(CredentialAgent::Opencode).unwrap(),
            "ci:one:opencode-api-key"
        );
    }

    /// Explicit because it writes one uniquely named disposable item to the
    /// current login keychain. Cleanup occurs before the assertion.
    #[test]
    #[ignore = "writes a disposable item to the current macOS login keychain"]
    fn writes_a_disposable_api_key_through_security_framework() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let item = format!("boundary-{}-{nonce}", process::id());
        let namespace = CredentialNamespace::new("ci".into(), Some(item)).unwrap();
        let store = LocalAgentCredentialStore::new(namespace);
        let account = store.account(CredentialAgent::Claude).unwrap();
        let key =
            AgentCredential::new(AgentCredentialKind::ApiKey, b"disposable-key".to_vec()).unwrap();

        store.save_api_key(CredentialAgent::Claude, &key).unwrap();
        let answer = get_generic_password(&ITEMS.service, &account);
        delete_generic_password(&ITEMS.service, &account).unwrap();

        assert_eq!(answer.unwrap(), b"disposable-key");
    }
}
