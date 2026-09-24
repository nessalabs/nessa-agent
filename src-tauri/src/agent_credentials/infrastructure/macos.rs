use nessa_agent_credentials::{
    AgentCredential, AgentCredentialKind, CredentialAgent, CredentialNamespace,
};
use security_framework::passwords::set_generic_password;

use crate::agent_credentials::{
    application::{AgentCredentialStore, CredentialSaveTargets, CredentialStoreFailure},
    domain::value_objects::CredentialSaveTarget,
    infrastructure::CanonicalCredentialSaveTargets,
};

/// The macOS login-keychain writer selected by desktop composition.
pub struct LocalAgentCredentialStore {
    targets: CanonicalCredentialSaveTargets,
}

impl LocalAgentCredentialStore {
    /// Store credentials under the exact durable service namespace.
    pub fn new(namespace: CredentialNamespace) -> Self {
        Self {
            targets: CanonicalCredentialSaveTargets::new(namespace),
        }
    }

    fn target_for(
        &self,
        agent: CredentialAgent,
    ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
        self.targets.target(agent)
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
