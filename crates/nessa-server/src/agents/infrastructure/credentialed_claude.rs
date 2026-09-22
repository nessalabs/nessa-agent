//! Claude provider construction with a credential read at each process launch.
//!
//! The source is the same injected port readiness asks. Its blocking keychain
//! read runs off the async executor, and an unavailable or malformed store
//! fails the open before any provider process can be started.

use std::{collections::BTreeMap, ffi::OsString, sync::Arc, time::Duration};

use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
        executions::ExecutionAudit,
        providers::{AgentProvider, ProviderIdentity, ProviderOpenError, ProviderOpenFuture},
    },
    domain::{
        agent_execution::{prompts::SystemPrompt, sessions::ExecutionSessionId},
        common::value_objects::TokenLimits,
        model_metadata::entities::ModelMetadata,
    },
    infrastructure::{acp::sessions::AcpConfig, claude_acp::sessions::ClaudeAcpProvider},
};

use crate::agents::{
    application::{AgentCredentialFailure, AgentCredentialKind, AgentCredentialSource},
    domain::AgentId,
};

const CREDENTIAL_READ_DEADLINE: Duration = Duration::from_secs(3);

/// A Claude ACP factory that reads the current credential for every open.
pub struct CredentialedClaudeProvider {
    config: AcpConfig,
    model: ModelMetadata,
    limits: TokenLimits,
    audit: Arc<dyn ExecutionAudit>,
    prompt: SystemPrompt,
    credentials: Arc<dyn AgentCredentialSource>,
    identity: ProviderIdentity,
}

impl CredentialedClaudeProvider {
    /// Validate immutable Claude configuration without reading a credential.
    ///
    /// # Errors
    /// Returns the same typed configuration failures as
    /// [`ClaudeAcpProvider::new`]. No credential store or process is touched.
    pub fn new(
        config: AcpConfig,
        model: ModelMetadata,
        limits: TokenLimits,
        audit: Arc<dyn ExecutionAudit>,
        prompt: SystemPrompt,
        credentials: Arc<dyn AgentCredentialSource>,
    ) -> Result<Self, AgentError> {
        let identity = ClaudeAcpProvider::new(config.clone(), &model, limits, audit.clone())?
            .with_system_prompt(prompt.clone())
            .identity();
        Ok(Self {
            config,
            model,
            limits,
            audit,
            prompt,
            credentials,
            identity,
        })
    }
}

impl AgentProvider for CredentialedClaudeProvider {
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }

    fn open(&self, restore: Option<ExecutionSessionId>) -> ProviderOpenFuture<'_> {
        let credentials = self.credentials.clone();
        let mut config = self.config.clone();
        let model = self.model.clone();
        let limits = self.limits;
        let audit = self.audit.clone();
        let prompt = self.prompt.clone();
        Box::pin(async move {
            let environment =
                read_credential_environment(credentials, CREDENTIAL_READ_DEADLINE).await?;
            config.credential_environment.extend(environment);
            let provider = ClaudeAcpProvider::new(config, &model, limits, audit)
                .map_err(ProviderOpenError::no_resources)?
                .with_system_prompt(prompt);
            provider.open(restore).await
        })
    }
}

async fn read_credential_environment(
    credentials: Arc<dyn AgentCredentialSource>,
    deadline: Duration,
) -> Result<BTreeMap<OsString, OsString>, ProviderOpenError> {
    tokio::time::timeout(
        deadline,
        tokio::task::spawn_blocking(move || credential_environment(credentials.as_ref())),
    )
    .await
    .map_err(|_| credential_open_failure(AgentCredentialFailure::Unavailable))?
    .map_err(|_| credential_open_failure(AgentCredentialFailure::Unavailable))?
    .map_err(credential_open_failure)
}

fn credential_environment(
    source: &dyn AgentCredentialSource,
) -> Result<BTreeMap<OsString, OsString>, AgentCredentialFailure> {
    let mut environment = BTreeMap::new();
    if let Some(credential) = source.read(AgentId::Claude)? {
        let variable = match credential.kind() {
            AgentCredentialKind::ApiKey => "ANTHROPIC_API_KEY",
            AgentCredentialKind::OAuthToken => "CLAUDE_CODE_OAUTH_TOKEN",
        };
        environment.insert(variable.into(), credential.expose().into());
    }
    Ok(environment)
}

fn credential_open_failure(failure: AgentCredentialFailure) -> ProviderOpenError {
    let detail = match failure {
        AgentCredentialFailure::Unavailable => "Claude credential store is unavailable",
        AgentCredentialFailure::Invalid => "Claude credential store returned an invalid value",
    };
    ProviderOpenError::no_resources(AgentError::Configuration(detail.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::application::AgentCredential;
    use std::thread;

    struct Credentials(Result<Option<(AgentCredentialKind, Vec<u8>)>, AgentCredentialFailure>);

    impl AgentCredentialSource for Credentials {
        fn read(&self, _: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
            match &self.0 {
                Ok(Some((kind, secret))) => AgentCredential::new(*kind, secret.clone())
                    .map(Some)
                    .map_err(|_| AgentCredentialFailure::Invalid),
                Ok(None) => Ok(None),
                Err(failure) => Err(*failure),
            }
        }
    }

    struct SlowCredentials;

    impl AgentCredentialSource for SlowCredentials {
        fn read(&self, _: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
            thread::sleep(Duration::from_millis(25));
            Ok(None)
        }
    }

    #[test]
    fn api_keys_and_oauth_tokens_keep_their_distinct_launch_meaning() {
        for (kind, variable) in [
            (AgentCredentialKind::ApiKey, "ANTHROPIC_API_KEY"),
            (AgentCredentialKind::OAuthToken, "CLAUDE_CODE_OAUTH_TOKEN"),
        ] {
            let environment =
                credential_environment(&Credentials(Ok(Some((kind, b"private".to_vec())))))
                    .unwrap();
            assert_eq!(environment.len(), 1);
            assert_eq!(
                environment.get(&OsString::from(variable)).unwrap(),
                "private"
            );
        }
    }

    #[test]
    fn absent_and_unavailable_credentials_remain_different_answers() {
        assert!(credential_environment(&Credentials(Ok(None)))
            .unwrap()
            .is_empty());
        assert_eq!(
            credential_environment(&Credentials(Err(AgentCredentialFailure::Unavailable))),
            Err(AgentCredentialFailure::Unavailable)
        );
    }

    #[tokio::test]
    async fn a_blocked_credential_store_cannot_block_provider_opening() {
        let error =
            read_credential_environment(Arc::new(SlowCredentials), Duration::from_millis(1))
                .await
                .unwrap_err();

        assert!(matches!(error.cause(), AgentError::Configuration(_)));
    }
}
