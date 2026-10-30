//! Claude provider construction with a credential read at each process launch.
//!
//! Its injected source runs blocking keychain reads off the async executor. An
//! unavailable or malformed store fails the open before a provider process can
//! be started. Composition selection remains outside this adapter.
//! Native-context classification delegates through the credential-free Claude
//! configuration; its query does not read the credential source.
//!
//! Deleting Claude's own record of a session launches Claude the same way, with
//! the credential read at that launch, over the binding's own deletion
//! exchange; every binding it launched for that is kept until it settles.

use nessa_sdk::application::agent_execution::providers::{HarnessHost, HarnessHostIdentity};
use std::{
    collections::BTreeMap, ffi::OsString, future::Future, mem, pin::Pin, sync::Arc, time::Duration,
};

use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
        executions::ExecutionAudit,
        providers::{
            AgentProvider, ApprovalMode, NativeHooksSwitch, ProviderIdentity, ProviderOpenError,
            ProviderOpenFuture, ProviderOpenRequest, ProviderSessionDeleter,
            ProviderSessionDeletionFuture,
        },
        traces::LocalTraceStore,
    },
    domain::{
        agent_execution::{prompts::SystemPrompt, sessions::ExecutionSessionId},
        common::value_objects::TokenLimits,
        effective_capabilities::value_objects::EffectiveCapabilities,
        model_metadata::{entities::ModelMetadata, value_objects::EffortLevel},
    },
    infrastructure::{acp::sessions::AcpConfig, claude_acp::sessions::ClaudeAcpProvider},
};

use crate::agents::application::{
    AgentCredentialFailure, AgentCredentialKind, AgentCredentialSource,
};
use crate::conversation::infrastructure::LaunchedDeletions;
use nessa_protocol::agents::AgentId;

const CREDENTIAL_READ_DEADLINE: Duration = Duration::from_secs(3);

/// A Claude ACP factory that reads the current credential for every open.
#[derive(Clone)]
pub struct CredentialedClaudeProvider {
    config: AcpConfig,
    model: ModelMetadata,
    limits: TokenLimits,
    audit: Arc<dyn ExecutionAudit>,
    prompt: SystemPrompt,
    approval_mode: ApprovalMode,
    effort_level: Option<EffortLevel>,
    native_hooks: NativeHooksSwitch,
    credentials: Arc<dyn AgentCredentialSource>,
    identity: ProviderIdentity,
    launch: String,
    capabilities: EffectiveCapabilities,
    /// The store every binding this factory builds records into.
    traces: Arc<LocalTraceStore>,
    /// Every binding a deletion launched, kept until it has settled.
    deleting: Arc<LaunchedDeletions<ClaudeAcpProvider>>,
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
        let launch = provider.launch_fingerprint();
        let capabilities = provider.capabilities().clone();
        Ok(Self {
            config,
            model,
            limits,
            audit,
            prompt,
            approval_mode: ApprovalMode::Ask,
            effort_level: None,
            native_hooks: NativeHooksSwitch::off(),
            credentials,
            identity,
            launch,
            capabilities,
            traces: LocalTraceStore::new(),
            deleting: Arc::new(LaunchedDeletions::default()),
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

    /// Let Claude run its own hooks when a process start reads the switch on.
    /// Off keeps today's suppression request. A deletion's process start
    /// suppresses hooks whatever the switch says.
    pub fn with_native_hooks(mut self, native_hooks: NativeHooksSwitch) -> Self {
        self.native_hooks = native_hooks;
        self
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
        self.configured(config)
            .map_err(ProviderOpenError::no_resources)
    }

    fn configured(&self, config: AcpConfig) -> Result<ClaudeAcpProvider, AgentError> {
        let provider =
            ClaudeAcpProvider::new(config, &self.model, self.limits, self.audit.clone())?
                .with_system_prompt(self.prompt.clone())
                .with_native_hooks(self.native_hooks.clone())
                .with_traces(self.traces.clone())
                .with_approval_mode(self.approval_mode)?;
        match self.effort_level.clone() {
            Some(level) => provider.with_effort_level(level),
            None => Ok(provider),
        }
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
    fn restores_saved_context(&self, saved: &ExecutionSessionId, has_invocations: bool) -> bool {
        // Classification uses only immutable configuration. A configuration
        // failure cannot authorize reset; opening reports its typed failure.
        self.configured(self.config.clone())
            .map_or(true, |provider| {
                provider.restores_saved_context(saved, has_invocations)
            })
    }

    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        HarnessHostIdentity::local()
    }
    fn effort_level(&self) -> Option<EffortLevel> {
        self.effort_level.clone()
    }
    fn model_change_context(&self) -> Option<String> {
        self.configured(self.config.clone())
            .ok()?
            .model_change_context()
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        let mode = mode.ok_or_else(|| {
            AgentError::Configuration("model change omitted approval mode".into())
        })?;
        let mut configured = self.clone().with_approval_mode(mode)?;
        configured.effort_level = effort;
        if let Some(traces) = traces {
            configured.traces = traces;
        }
        configured.configured(configured.config.clone())?;
        Ok(Arc::new(configured))
    }

    fn approval_mode(&self) -> Option<ApprovalMode> {
        Some(self.approval_mode)
    }

    fn native_hooks_allowed(&self) -> bool {
        self.native_hooks.allowed()
    }

    fn launch_fingerprint(&self) -> String {
        self.launch.clone()
    }
    fn identity(&self) -> ProviderIdentity {
        self.identity.clone()
    }

    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }

    fn traces(&self) -> Option<Arc<LocalTraceStore>> {
        Some(self.traces.clone())
    }

    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async move { self.current().await?.open(request).await })
    }

    /// Claude on another machine runs with that machine's own sign-in: the
    /// credential this gateway reads is never sent, so the binding there is
    /// the plain one its host starts.
    fn on_host(&self, host: Arc<dyn HarnessHost>) -> Result<Arc<dyn AgentProvider>, AgentError> {
        self.configured(self.config.clone())?.on_host(host)
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
    use std::{
        path::Path,
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, Mutex,
        },
    };
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

    fn fixture_provider(
        root: &Path,
        credentials: Arc<dyn AgentCredentialSource>,
    ) -> CredentialedClaudeProvider {
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
            staged_runtime_arguments: Vec::new(),
            environment: BTreeMap::new(),
            runtime_environment: BTreeMap::new(),
            credential_environment: BTreeMap::new(),
            open_credentials: Default::default(),
            workspace: root.into(),
            tools_enabled: true,
            mcp_servers: nessa_sdk::infrastructure::acp::sessions::McpServerList::none(),
            stand_ins: nessa_sdk::infrastructure::acp::sessions::StandInSessions::none(),
            permissions: PermissionOfferPolicy::once_only(),
            refusal: nessa_sdk::application::agent_execution::permissions::ConfiguredRefusal::continue_turn(),
            permission_deadlines: nessa_sdk::domain::agent_execution::permissions::PermissionDeadlinePolicy::standard(),
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
            workspace_paths: Arc::new(nessa_sdk::infrastructure::workspace_paths::FilesystemPaths),
            permission_rules: std::sync::Arc::new(nessa_sdk::application::agent_execution::permissions::IdlePermissionRules),
            review_notices: std::sync::Arc::new(nessa_sdk::application::agent_execution::permissions::IdleReviewNotices),
        };
        let prompt = SystemPrompt::new(vec![PromptContribution::new(
            PromptSource::new(PromptSourceKind::Core, "test").unwrap(),
            "Be concise.",
        )])
        .unwrap();
        CredentialedClaudeProvider::new(
            config,
            model,
            TokenLimits::new(100_000, 32_000).unwrap(),
            Arc::new(crate::conversation_test_support::AcceptingAudit),
            prompt,
            credentials,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn the_credential_wrapper_reports_the_mode_it_launches() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Arc::new(Credentials(Ok(None)));
        let provider = fixture_provider(root.path(), credentials)
            .with_approval_mode(ApprovalMode::Auto)
            .unwrap();
        assert_eq!(provider.approval_mode(), Some(ApprovalMode::Auto));
        let context = provider
            .model_change_context()
            .expect("credential wrapper publishes the native context proof");
        let traces = LocalTraceStore::new();
        let normalized = provider
            .with_model_configuration(Some(ApprovalMode::Ask), None, Some(traces.clone()))
            .unwrap();
        assert_eq!(normalized.identity(), provider.identity());
        assert_eq!(normalized.model_change_context().as_ref(), Some(&context));
        assert_eq!(normalized.approval_mode(), Some(ApprovalMode::Ask));
        assert_eq!(normalized.capabilities(), provider.capabilities());
        assert!(Arc::ptr_eq(&normalized.traces().unwrap(), &traces));

        let first = provider.current().await.unwrap();
        let second = provider.current().await.unwrap();
        assert_eq!(first.approval_mode(), Some(ApprovalMode::Auto));
        assert_eq!(second.approval_mode(), Some(ApprovalMode::Auto));
        let store = provider.traces().unwrap();
        assert!(Arc::ptr_eq(&store, &first.traces().unwrap()));
        assert!(Arc::ptr_eq(&store, &second.traces().unwrap()));
    }

    struct CountingCredentials {
        reads: AtomicUsize,
        failure: Option<AgentCredentialFailure>,
    }
    impl AgentCredentialSource for CountingCredentials {
        fn read(&self, _: AgentId) -> Result<Option<AgentCredential>, AgentCredentialFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.failure.map_or(Ok(None), Err)
        }
    }

    #[test]
    fn context_restoration_delegates_without_reading_credentials() {
        let root = tempfile::tempdir().unwrap();
        let saved = ExecutionSessionId::new("saved-native-context").unwrap();
        for failure in [None, Some(AgentCredentialFailure::Unavailable)] {
            let credentials = Arc::new(CountingCredentials {
                reads: AtomicUsize::new(0),
                failure,
            });
            let provider = fixture_provider(root.path(), credentials.clone());
            let claude = provider.configured(provider.config.clone()).unwrap();
            for admitted in [false, true] {
                assert_eq!(claude.restores_saved_context(&saved, admitted), admitted);
                assert_eq!(
                    provider.restores_saved_context(&saved, admitted),
                    claude.restores_saved_context(&saved, admitted)
                );
            }
            assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn invalid_context_configuration_cannot_authorize_native_history_reset() {
        let root = tempfile::tempdir().unwrap();
        let credentials = Arc::new(CountingCredentials {
            reads: AtomicUsize::new(0),
            failure: None,
        });
        let mut provider = fixture_provider(root.path(), credentials.clone());
        provider.config.event_capacity = 0;
        assert!(matches!(
            provider.configured(provider.config.clone()),
            Err(AgentError::Configuration(_))
        ));
        let saved = ExecutionSessionId::new("saved-native-context").unwrap();
        for admitted in [false, true] {
            assert!(provider.restores_saved_context(&saved, admitted));
        }
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
    }

    /// Reuse the SDK's local Claude ACP contract fixture rather than defining a
    /// second native protocol. Its system-prompt mode checks this wrapper's
    /// instructions; removing its empty record models Claude not persisting an
    /// unused context. No credential or real Claude process is needed.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unused_credentialed_claude_context_reopens_without_native_history() {
        use nessa_sdk::{
            application::agent_execution::{
                agents::{Agent, AttachmentRequest},
                permissions::ActionContext,
                sessions::{SessionManager, SessionStorage},
            },
            domain::agent_execution::{prompts::SystemPromptBuilder, sessions::SessionId},
            infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock},
        };

        fn actor(request: &str) -> ActionContext {
            ActionContext::new("fixture-host", "fixture-tests", request).unwrap()
        }
        async fn attach(
            provider: Arc<CredentialedClaudeProvider>,
            storage: Arc<RecordStorage>,
            id: SessionId,
            request: &str,
        ) -> Agent {
            let manager = SessionManager::open(
                Some(id),
                storage,
                Arc::new(RuntimeMessageCommitClock::new()),
            )
            .await
            .unwrap();
            let agent = Agent::prepare(
                provider,
                manager,
                Arc::new(crate::conversation_test_support::AcceptingAudit),
            )
            .await
            .unwrap();
            let authorization = agent
                .authorize_attachment(AttachmentRequest::CallerRequested(actor(request)))
                .unwrap();
            agent
                .start_attachment(authorization)
                .unwrap()
                .wait()
                .await
                .unwrap();
            agent
        }

        let root = tempfile::tempdir().unwrap();
        let credentials = Arc::new(CountingCredentials {
            reads: AtomicUsize::new(0),
            failure: None,
        });
        let template = fixture_provider(root.path(), credentials.clone());
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../nessa-sdk/tests/infrastructure/acp/contracts/fixtures/claude_acp_test_handler.py",
        );
        assert!(fixture.is_file(), "the shared SDK ACP fixture is present");
        let mut config = template.config.clone();
        config.executable = ExecutableUseSnapshot::unmanaged("/usr/bin/python3".into());
        config.arguments = vec![fixture.into_os_string(), "system-prompt".into()];
        let prompt = SystemPromptBuilder::new()
            .text(
                PromptSource::new(PromptSourceKind::Core, "nessa/system").unwrap(),
                "Core instructions.",
            )
            .text(
                PromptSource::new(PromptSourceKind::Plugin, "reviewer").unwrap(),
                "\nPlugin instructions.",
            )
            .build()
            .unwrap();
        let provider = Arc::new(
            CredentialedClaudeProvider::new(
                config,
                template.model,
                TokenLimits::new(900, 100).unwrap(),
                template.audit,
                prompt,
                credentials.clone(),
            )
            .unwrap(),
        );
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 0);
        let storage_root = tempfile::tempdir().unwrap();
        let path = storage_root.path().join("sessions");
        let id = SessionId::new("credentialed-unused-context").unwrap();
        let storage = Arc::new(RecordStorage::new(&path).unwrap());
        let first = attach(provider.clone(), storage.clone(), id.clone(), "open-first").await;
        let before = first.session_manager().snapshot().await.unwrap();
        assert!(before.invocations.is_empty());
        assert_eq!(before.provider_context.admission_watermark(), Some(0));
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 1);
        first.close(actor("close-first")).await.unwrap();
        drop(first);
        storage.shutdown().await.unwrap();
        std::fs::remove_file(root.path().join("saved-session")).unwrap();

        let storage = Arc::new(RecordStorage::new(&path).unwrap());
        let second = attach(provider, storage.clone(), id, "open-second").await;
        let after = second.session_manager().snapshot().await.unwrap();
        assert!(after.invocations.is_empty());
        assert_eq!(after.provider_context.admission_watermark(), Some(0));
        assert_ne!(
            after.provider_context.recorded(),
            before.provider_context.recorded()
        );
        assert!(!root.path().join("resume-observed").exists());
        assert!(!root.path().join("prompt-observed").exists());
        let native: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("saved-session")).unwrap())
                .unwrap();
        assert_eq!(native["history"].as_array().unwrap().len(), 0);
        assert_eq!(
            std::fs::read_to_string(root.path().join("new-session-count")).unwrap(),
            "2"
        );
        assert_eq!(credentials.reads.load(Ordering::SeqCst), 2);
        second.close(actor("close-second")).await.unwrap();
        drop(second);
        storage.shutdown().await.unwrap();
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
