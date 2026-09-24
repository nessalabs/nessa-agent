use std::sync::LazyLock;

use nessa_agent_credentials::{
    AgentCredential, AgentCredentialKind, CredentialAgent, CredentialNamespace,
};
use security_framework::passwords::set_generic_password;
use serde::Deserialize;

use crate::agent_credentials::{
    application::{AgentCredentialStore, CredentialStoreFailure},
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

/// The macOS login-keychain writer selected by desktop composition.
pub struct LocalAgentCredentialStore {
    namespace: CredentialNamespace,
}

impl LocalAgentCredentialStore {
    /// Store credentials under the exact durable service namespace.
    pub fn new(namespace: CredentialNamespace) -> Self {
        Self { namespace }
    }

    fn target_for(
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

impl AgentCredentialStore for LocalAgentCredentialStore {
    fn target(
        &self,
        agent: CredentialAgent,
    ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
        self.target_for(agent)
    }

    fn save_api_key(
        &self,
        target: &CredentialSaveTarget,
        credential: &AgentCredential,
    ) -> Result<(), CredentialStoreFailure> {
        if credential.kind() != AgentCredentialKind::ApiKey {
            return Err(CredentialStoreFailure::Invalid);
        }
        if self.target_for(target.agent())? != *target {
            return Err(CredentialStoreFailure::Invalid);
        }
        set_generic_password(
            target.service(),
            target.account(),
            credential.expose().as_bytes(),
        )
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
            store.target_for(CredentialAgent::Claude).unwrap().account(),
            "ci:one:claude-api-key"
        );
        assert_eq!(
            store
                .target_for(CredentialAgent::Opencode)
                .unwrap()
                .account(),
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
        let target = store.target_for(CredentialAgent::Claude).unwrap();
        let key =
            AgentCredential::new(AgentCredentialKind::ApiKey, b"disposable-key".to_vec()).unwrap();

        store.save_api_key(&target, &key).unwrap();
        let answer = get_generic_password(target.service(), target.account());
        delete_generic_password(target.service(), target.account()).unwrap();

        assert_eq!(answer.unwrap(), b"disposable-key");
    }
}
