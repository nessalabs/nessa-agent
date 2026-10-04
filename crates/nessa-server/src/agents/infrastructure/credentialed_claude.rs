//! Claude provider construction with a credential read at each process launch.
//!
//! Its injected source runs blocking keychain reads off the async executor. An
//! unavailable or malformed store fails the open before a provider process can
//! be started. Composition selection remains outside this adapter.
//!
//! Deleting Claude's own record of a session launches Claude the same way, with
//! the credential read at that launch, over the binding's own deletion
//! exchange; every binding it launched for that is kept until it settles.

use std::{
    collections::BTreeMap, ffi::OsString, future::Future, mem, pin::Pin, sync::Arc, time::Duration,
};

use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
        executions::ExecutionAudit,
        providers::{
            AgentProvider, ApprovalMode, ProviderIdentity, ProviderOpenError, ProviderOpenFuture,
            ProviderOpenRequest, ProviderSessionDeleter, ProviderSessionDeletionFuture,
        },
    },
    domain::{
        agent_execution::{prompts::SystemPrompt, sessions::ExecutionSessionId},
        common::value_objects::TokenLimits,
        effective_capabilities::value_objects::EffectiveCapabilities,
        model_metadata::entities::ModelMetadata,
    },
    infrastructure::{acp::sessions::AcpConfig, claude_acp::sessions::ClaudeAcpProvider},
};

use crate::agents::{
    application::{AgentCredentialFailure, AgentCredentialKind, AgentCredentialSource},
    domain::AgentId,
};
use crate::conversation::infrastructure::LaunchedDeletions;

const CREDENTIAL_READ_DEADLINE: Duration = Duration::from_secs(3);

/// A Claude ACP factory that reads the current credential for every open.
pub struct CredentialedClaudeProvider {
    config: AcpConfig,
    model: ModelMetadata,
    limits: TokenLimits,
    audit: Arc<dyn ExecutionAudit>,
    prompt: SystemPrompt,
    approval_mode: ApprovalMode,
    credentials: Arc<dyn AgentCredentialSource>,
    identity: ProviderIdentity,
    /// What [`ClaudeAcpProvider::previous_identity`] answers for this
    /// configuration, for the one-shot fingerprint retrofit alone.
    previous_identity: ProviderIdentity,
    capabilities: EffectiveCapabilities,
    /// Every binding a deletion launched, kept until it has settled.
    deleting: LaunchedDeletions<ClaudeAcpProvider>,
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
        let provider = ClaudeAcpProvider::new(config.clone(), &model, limits, audit.clone())?
            .with_system_prompt(prompt.clone());
        let identity = provider.identity();
        let previous_identity = provider.previous_identity();
        let capabilities = provider.capabilities().clone();
        Ok(Self {
            config,
            model,
            limits,
            audit,
            prompt,
            approval_mode: ApprovalMode::Ask,
            credentials,
            identity,
            previous_identity,
            capabilities,
            deleting: LaunchedDeletions::default(),
        })
    }

    /// Select one preset verified by the underlying Claude binding for this model.
    ///
    /// # Errors
    /// Returns [`AgentError::Unsupported`] when the preset has not been verified.
    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Result<Self, AgentError> {
        ClaudeAcpProvider::approval_modes(self.model.key().model_id())
            .iter()
            .any(|choice| choice.id == mode)
            .then_some(())
            .ok_or_else(|| {
                AgentError::Unsupported(
                    "Claude approval preset is unavailable for this model".into(),
                )
            })?;
        self.approval_mode = mode;
        Ok(self)
    }

    /// The identity this provider had under the restoration fingerprint that
    /// still hashed MCP servers: [`ClaudeAcpProvider::previous_identity`] for
    /// the same configuration, which no credential reaches. Read only by the
    /// one-shot fingerprint retrofit, and removed with it in #471.
    pub fn previous_identity(&self) -> ProviderIdentity {
        self.previous_identity.clone()
    }

    /// A Claude binding launched with the credential current now.
    async fn current(&self) -> Result<ClaudeAcpProvider, ProviderOpenError> {
        let mut config = self.config.clone();
        let inherited_environment = mem::take(&mut config.credential_environment);
        config.credential_environment = read_credential_environment(
            inherited_environment,
            self.credentials.clone(),
            CREDENTIAL_READ_DEADLINE,
        )
        .await?;
        ClaudeAcpProvider::new(config, &self.model, self.limits, self.audit.clone())
            .map_err(ProviderOpenError::no_resources)?
            .with_system_prompt(self.prompt.clone())
            .with_approval_mode(self.approval_mode)
            .map_err(ProviderOpenError::no_resources)
    }
}

impl ProviderSessionDeleter for CredentialedClaudeProvider {
    fn delete_session(&self, session: ExecutionSessionId) -> ProviderSessionDeletionFuture<'_> {
        Box::pin(async move {
            let binding = Arc::new(
                self.current()
                    .await
                    .map_err(|failure| failure.cause().clone())?,
            );
            self.deleting.push(binding.clone());
            binding.delete_session(session).await
        })
    }
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.deleting.settled())
    }
    fn cleanup_outstanding(&self) -> bool {
        self.deleting.cleanup_outstanding()
    }
}

impl AgentProvider for CredentialedClaudeProvider {
    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }

    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }

    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move { self.current().await?.open(request).await })
    }
}

async fn read_credential_environment(
    environment: BTreeMap<OsString, OsString>,
    credentials: Arc<dyn AgentCredentialSource>,
    deadline: Duration,
) -> Result<BTreeMap<OsString, OsString>, ProviderOpenError> {
    tokio::time::timeout(
        deadline,
        tokio::task::spawn_blocking(move || {
            credential_environment(environment, credentials.as_ref())
        }),
    )
    .await
    .map_err(|_| credential_open_failure(AgentCredentialFailure::Unavailable))?
    .map_err(|_| credential_open_failure(AgentCredentialFailure::Unavailable))?
    .map_err(credential_open_failure)
}

pub(super) fn credential_environment(
    mut environment: BTreeMap<OsString, OsString>,
    source: &dyn AgentCredentialSource,
) -> Result<BTreeMap<OsString, OsString>, AgentCredentialFailure> {
    environment.remove(&OsString::from("ANTHROPIC_API_KEY"));
    environment.remove(&OsString::from("CLAUDE_CODE_OAUTH_TOKEN"));
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
    use nessa_sdk::{
        application::{
            agent_execution::providers::ExecutableUseSnapshot,
            dto::{ModalitiesDto, ModelMetadataDto, ReasoningDto},
        },
        domain::agent_execution::{
            permissions::PermissionOfferPolicy,
            prompts::{PromptContribution, PromptSource, PromptSourceKind},
        },
        infrastructure::clock::RuntimeClock,
    };
    use std::sync::{mpsc, Mutex};
    use tokio::sync::oneshot;

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

    struct BlockingCredentials {
        entered: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl AgentCredentialSource for BlockingCredentials {
        fn read(&self, _: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            let _ = self.release.lock().unwrap().recv();
            Ok(None)
        }
    }

    #[tokio::test]
    async fn the_credential_wrapper_reports_the_mode_it_launches() {
        let root = tempfile::tempdir().unwrap();
        let text = ModalitiesDto {
            text: true,
            image: false,
            audio: false,
        };
        let model = ModelMetadata::try_from(ModelMetadataDto {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-5".into(),
            display_name: "Sonnet 5".into(),
            input: text,
            image_input: None,
            output: text,
            tool_use: true,
            reasoning: Some(ReasoningDto {
                effort_levels: vec![],
            }),
            fast_mode: false,
            max_context_window_tokens: 1_000_000,
            max_output_tokens: 32_000,
            knowledge_cutoff: "2026-01".into(),
            documentation_url: "https://example.com".into(),
        })
        .unwrap();
        let config = AcpConfig {
            executable: ExecutableUseSnapshot::unmanaged("/bin/true".into()),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            credential_environment: BTreeMap::new(),
            workspace: root.path().into(),
            tools_enabled: true,
            mcp_servers: Vec::new(),
            stand_ins: nessa_sdk::infrastructure::acp::sessions::StandInSessions::none(),
            permissions: PermissionOfferPolicy::once_only(),
            launch_timeout: Duration::from_secs(10),
            startup_timeout: Duration::from_secs(10),
            execution_timeout: None,
            shutdown_grace: Duration::from_secs(1),
            kill_timeout: Duration::from_secs(1),
            event_capacity: 16,
            max_frame_bytes: 8192,
            max_incoming_frame_bytes: 8192,
            images: None,
            clock: Arc::new(RuntimeClock::new()),
        };
        let prompt = SystemPrompt::new(vec![PromptContribution::new(
            PromptSource::new(PromptSourceKind::Core, "test").unwrap(),
            "Be concise.",
        )])
        .unwrap();
        let credentials = Arc::new(Credentials(Ok(None)));
        let provider = CredentialedClaudeProvider::new(
            config,
            model,
            TokenLimits::new(100_000, 32_000).unwrap(),
            Arc::new(crate::conversation_test_support::AcceptingAudit),
            prompt,
            credentials,
        )
        .unwrap()
        .with_approval_mode(ApprovalMode::Auto)
        .unwrap();
        assert_eq!(provider.approval_mode(), Some(ApprovalMode::Auto));
        assert_eq!(
            provider.current().await.unwrap().approval_mode(),
            Some(ApprovalMode::Auto)
        );
    }

    #[test]
    fn api_keys_and_oauth_tokens_keep_their_distinct_launch_meaning() {
        for (kind, variable) in [
            (AgentCredentialKind::ApiKey, "ANTHROPIC_API_KEY"),
            (AgentCredentialKind::OAuthToken, "CLAUDE_CODE_OAUTH_TOKEN"),
        ] {
            let environment = credential_environment(
                BTreeMap::new(),
                &Credentials(Ok(Some((kind, b"private".to_vec())))),
            )
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
        assert!(
            credential_environment(BTreeMap::new(), &Credentials(Ok(None)))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            credential_environment(
                BTreeMap::new(),
                &Credentials(Err(AgentCredentialFailure::Unavailable)),
            ),
            Err(AgentCredentialFailure::Unavailable)
        );
    }

    #[test]
    fn the_source_replaces_every_inherited_claude_auth_combination() {
        let base = |variable: &str| {
            BTreeMap::from([
                (OsString::from(variable), OsString::from("inherited")),
                (OsString::from("PATH"), OsString::from("/usr/bin")),
            ])
        };
        for (inherited, kind, expected) in [
            (
                "CLAUDE_CODE_OAUTH_TOKEN",
                AgentCredentialKind::ApiKey,
                "ANTHROPIC_API_KEY",
            ),
            (
                "ANTHROPIC_API_KEY",
                AgentCredentialKind::OAuthToken,
                "CLAUDE_CODE_OAUTH_TOKEN",
            ),
        ] {
            let environment = credential_environment(
                base(inherited),
                &Credentials(Ok(Some((kind, b"current".to_vec())))),
            )
            .unwrap();
            assert_eq!(
                environment.get(&OsString::from(expected)).unwrap(),
                "current"
            );
            assert!(!environment.contains_key(&OsString::from(inherited)));
            assert_eq!(
                environment.get(&OsString::from("PATH")).unwrap(),
                "/usr/bin"
            );
        }

        let environment = credential_environment(
            BTreeMap::from([
                (
                    OsString::from("ANTHROPIC_API_KEY"),
                    OsString::from("inherited-key"),
                ),
                (
                    OsString::from("CLAUDE_CODE_OAUTH_TOKEN"),
                    OsString::from("inherited-token"),
                ),
            ]),
            &Credentials(Ok(None)),
        )
        .unwrap();
        assert!(environment.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_credential_store_cannot_block_provider_opening() {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let credentials = Arc::new(BlockingCredentials {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
        });
        let read = tokio::spawn(read_credential_environment(
            BTreeMap::new(),
            credentials,
            Duration::from_secs(3),
        ));
        entered_rx.await.unwrap();
        tokio::time::advance(Duration::from_secs(3)).await;

        let error = read.await.unwrap().unwrap_err();

        assert!(matches!(error.cause(), AgentError::Configuration(_)));
        release_tx.send(()).unwrap();
    }
}
