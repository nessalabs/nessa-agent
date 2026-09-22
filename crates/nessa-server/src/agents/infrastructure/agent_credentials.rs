//! Nessa-owned credentials supplied to local agent launches.
//!
//! Environment credentials remain an explicit standalone-server input. A
//! packaged service can instead read a stage-scoped Nessa keychain item once
//! composition injects this source into its consumers.

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::{collections::HashMap, ffi::OsString, sync::LazyLock};

use nessa_agent_credentials::{CredentialAgent, CredentialNamespace};
use serde::Deserialize;

use crate::agents::{
    application::{
        AgentCredential, AgentCredentialFailure, AgentCredentialKind, AgentCredentialSource,
    },
    domain::AgentId,
};

const CREDENTIALS_JSON: &str =
    include_str!("../../../../../protocol/defaults/agent-credentials.json");

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

trait KeychainReader: Send + Sync {
    fn read(&self, service: &str, account: &str)
        -> Result<Option<Vec<u8>>, AgentCredentialFailure>;
}

/// A credential source available for server composition to select.
pub struct LocalAgentCredentials {
    environment: HashMap<CredentialAgent, (AgentCredentialKind, OsString)>,
    keychain: Box<dyn KeychainReader>,
    namespace: CredentialNamespace,
}

impl LocalAgentCredentials {
    /// Capture standalone environment credentials and the durable keychain namespace.
    pub fn from_environment(namespace: CredentialNamespace) -> Self {
        let mut environment = HashMap::new();
        if let Some(value) = std::env::var_os("ANTHROPIC_API_KEY") {
            environment.insert(
                CredentialAgent::Claude,
                (AgentCredentialKind::ApiKey, value),
            );
        } else if let Some(value) = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN") {
            environment.insert(
                CredentialAgent::Claude,
                (AgentCredentialKind::OAuthToken, value),
            );
        }
        Self {
            environment,
            keychain: platform_keychain(),
            namespace,
        }
    }

    fn account(&self, agent: CredentialAgent) -> Result<String, AgentCredentialFailure> {
        let item = match agent {
            CredentialAgent::Claude => &ITEMS.accounts.claude,
            CredentialAgent::Opencode => &ITEMS.accounts.opencode,
        };
        self.namespace
            .account(item)
            .map_err(|_| AgentCredentialFailure::Invalid)
    }
}

impl AgentCredentialSource for LocalAgentCredentials {
    fn read(&self, agent: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
        let agent = match agent {
            AgentId::Claude => CredentialAgent::Claude,
            AgentId::Opencode => CredentialAgent::Opencode,
            AgentId::Codex => return Ok(None),
        };
        if let Some((kind, value)) = self.environment.get(&agent) {
            let bytes = os_bytes(value)?;
            return credential(*kind, bytes).map(Some);
        }
        let account = self.account(agent)?;
        self.keychain
            .read(&ITEMS.service, &account)?
            .map(|secret| credential(AgentCredentialKind::ApiKey, secret))
            .transpose()
    }
}

fn credential(
    kind: AgentCredentialKind,
    secret: Vec<u8>,
) -> Result<AgentCredential, AgentCredentialFailure> {
    AgentCredential::new(kind, secret).map_err(|_| AgentCredentialFailure::Invalid)
}

#[cfg(unix)]
fn os_bytes(value: &OsString) -> Result<Vec<u8>, AgentCredentialFailure> {
    Ok(value.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn os_bytes(value: &OsString) -> Result<Vec<u8>, AgentCredentialFailure> {
    value
        .to_str()
        .map(|value| value.as_bytes().to_vec())
        .ok_or(AgentCredentialFailure::Invalid)
}

#[cfg(target_os = "macos")]
struct LoginKeychain;

#[cfg(target_os = "macos")]
impl KeychainReader for LoginKeychain {
    fn read(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, AgentCredentialFailure> {
        super::agent_credentials_macos::read(service, account)
    }
}

#[cfg(not(target_os = "macos"))]
struct LoginKeychain;

#[cfg(not(target_os = "macos"))]
impl KeychainReader for LoginKeychain {
    fn read(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, AgentCredentialFailure> {
        Ok(None)
    }
}

fn platform_keychain() -> Box<dyn KeychainReader> {
    Box::new(LoginKeychain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Keychain {
        answer: Mutex<Result<Option<Vec<u8>>, AgentCredentialFailure>>,
    }
    impl KeychainReader for Keychain {
        fn read(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, AgentCredentialFailure> {
            self.answer.lock().unwrap().clone()
        }
    }

    fn source(answer: Result<Option<Vec<u8>>, AgentCredentialFailure>) -> LocalAgentCredentials {
        LocalAgentCredentials {
            environment: HashMap::new(),
            keychain: Box::new(Keychain {
                answer: Mutex::new(answer),
            }),
            namespace: CredentialNamespace::new("ci".into(), Some("one".into())).unwrap(),
        }
    }

    #[test]
    fn the_same_source_returns_a_stage_and_instance_scoped_api_key() {
        let source = source(Ok(Some(b"secret".to_vec())));
        let credential = source.read(AgentId::Claude).unwrap().unwrap();
        assert_eq!(credential.kind(), AgentCredentialKind::ApiKey);
        assert_eq!(credential.expose(), "secret");
        assert_eq!(
            source.account(CredentialAgent::Claude).unwrap(),
            "ci:one:claude-api-key"
        );
    }

    #[test]
    fn an_unavailable_or_invalid_store_never_claims_a_sign_in() {
        for failure in [
            AgentCredentialFailure::Unavailable,
            AgentCredentialFailure::Invalid,
        ] {
            assert_eq!(
                source(Err(failure)).read(AgentId::Claude).err(),
                Some(failure)
            );
        }
        assert_eq!(
            source(Ok(Some(Vec::new()))).read(AgentId::Claude).err(),
            Some(AgentCredentialFailure::Invalid)
        );
    }
}
