//! Current agent observations for readiness and cold conversation opening.
//!
//! Composition owns the relationship between static OpenCode policy, launch
//! evidence, credentials, and provider construction. Readiness and a cold slot
//! each take a fresh observation; packaged observations read the managed store
//! and stage-scoped credential source, while standalone observations retain the
//! explicit launch and environment credential captured when composition began.
//! A live conversation keeps the provider generation it already owns.
//!
//! ```text
//! readiness ─┐
//!            ├─▶ CurrentAgentResolver ─▶ effective OpenCode profile
//! cold slot ─┘                         ├─▶ fresh managed evidence, when packaged
//!                                      ├─▶ owned provider generation
//!                                      └─▶ automatic warm-up lane
//! ```
//!
//! Arrows are calls. OpenCode's blocking observation has one bounded lane.
//! Deleting OpenCode's own record of a session observes the current generation
//! the same way, in the same lane, and asks that generation's binding
//! ([`CurrentAgentEraser`]). The
//! blocking task owns its permit through actual completion, even when its
//! awaiting caller times out or is dropped. A different warm-up fingerprint
//! waits without retaining that observation, then resolves every external fact
//! again after the active run confirms physical release.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::providers::{
    ApprovalMode, ExecutableUseGuard, ExecutableUseSnapshot, UserImageSource,
};
use nessa_sdk::infrastructure::{
    claude_acp::sessions::ClaudeAcpProvider, codex_acp::sessions::CodexAcpProvider,
};
use tokio::sync::Semaphore;

use super::{
    agent::{self, AgentRuntime, AgentsConfig, ProviderDependencies},
    installed_launch::{installed_launch, supports_installed_launch, InstalledLaunch},
    managed_adapter::ManagedAdapter,
    opencode_profile::{
        captured_credential_environment, EffectiveOpenCodeProfile, OpenCodeCredentialMode,
        OpenCodeProfile,
    },
    warm_up::{CurrentOpenCodeWarmUp, CurrentWarmUpAdmission},
};
use crate::{
    agent_install::{application::RuntimeStore, domain::HostPlatform},
    agents::{
        application::{
            AgentCredentialKind, AgentCredentialSource, AgentProbe, AgentProbeEvidence,
            ProbeFailure,
        },
        infrastructure::{AgentLaunchFiles, LocalAgentProbe},
    },
    conversation::{
        application::{
            ConversationAgent, ConversationAgentFuture, ConversationAgentSource, ConversationError,
            ConversationFuture, ProviderSessionEraser,
        },
        domain::ProviderSessionErasure,
    },
    core::RunError,
};
use nessa_protocol::{agents::AgentId, conversation::domain::ConversationApprovalMode};

const RESOLUTION_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct CurrentAgentResolver {
    fixed: HashMap<AgentId, ConversationAgent>,
    managed_adapters: HashSet<AgentId>,
    probe_cleanup: Arc<Mutex<HashMap<AgentId, ProbeCleanup>>>,
    fixed_probe: Arc<LocalAgentProbe>,
    config: AgentsConfig,
    opencode: Arc<EffectiveOpenCodeProfile>,
    store: Arc<dyn RuntimeStore>,
    host: HostPlatform,
    credentials: Arc<dyn AgentCredentialSource>,
    provider: ProviderDependencies,
    warm_up: CurrentOpenCodeWarmUp,
    /// Warm-up resolutions scheduled and not yet returned, shared by clones.
    warm_up_resolving: Arc<AtomicUsize>,
    slots: Arc<Semaphore>,
}

pub(super) struct CurrentAgentResolverInput {
    pub(super) fixed: HashMap<AgentId, ConversationAgent>,
    pub(super) managed_adapters: HashSet<AgentId>,
    pub(super) fixed_probe: LocalAgentProbe,
    pub(super) config: AgentsConfig,
    pub(super) opencode: EffectiveOpenCodeProfile,
    pub(super) store: Arc<dyn RuntimeStore>,
    pub(super) host: HostPlatform,
    pub(super) credentials: Arc<dyn AgentCredentialSource>,
    pub(super) provider_directory: PathBuf,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) images: Arc<dyn UserImageSource>,
    pub(super) warm_up: CurrentOpenCodeWarmUp,
}

struct Observation {
    evidence: Option<AgentProbeEvidence>,
    provider:
        Result<Option<ConversationAgent>, crate::conversation::application::ConversationError>,
}

impl CurrentAgentResolver {
    pub(super) fn new(input: CurrentAgentResolverInput) -> Self {
        Self {
            fixed: input.fixed,
            managed_adapters: input.managed_adapters,
            probe_cleanup: Arc::new(Mutex::new(HashMap::new())),
            fixed_probe: Arc::new(input.fixed_probe),
            config: input.config,
            opencode: Arc::new(input.opencode),
            store: input.store,
            host: input.host,
            credentials: input.credentials.clone(),
            provider: ProviderDependencies {
                directory: input.provider_directory,
                clock: input.clock,
                images: input.images,
                credentials: input.credentials,
            },
            warm_up: input.warm_up,
            warm_up_resolving: Arc::new(AtomicUsize::new(0)),
            slots: Arc::new(Semaphore::new(1)),
        }
    }

    pub(super) fn configured(&self) -> HashSet<AgentId> {
        let mut configured: HashSet<_> = self
            .fixed
            .keys()
            .copied()
            .chain(self.managed_adapters.iter().copied())
            .collect();
        if self.opencode.configured().is_some() {
            configured.insert(AgentId::Opencode);
        }
        configured
    }

    /// Register how OpenCode deletes its own record of a session, when it is
    /// configured here: by its current generation, observed on each ask
    /// (`opencode_is_registered_to_delete_sessions_only_when_configured`).
    pub(super) fn register_session_eraser(
        self: &Arc<Self>,
        erasers: &mut crate::conversation::application::ProviderSessionErasers,
    ) {
        if self.opencode.configured().is_some() {
            erasers.register(
                AgentId::Opencode,
                Arc::new(CurrentAgentEraser::new(self.clone(), AgentId::Opencode)),
            );
        }
        for agent in &self.managed_adapters {
            erasers.register(
                *agent,
                Arc::new(CurrentAgentEraser::new(self.clone(), *agent)),
            );
        }
    }

    pub(super) fn default_agent(&self) -> Result<AgentId, RunError> {
        self.config.selected_from(&self.configured())
    }

    fn managed_native(
        &self,
        agent: AgentId,
    ) -> Result<Option<ExecutableUseSnapshot>, ConversationError> {
        match installed_launch(agent, &self.host, self.store.as_ref()) {
            Ok(InstalledLaunch::Ready(native)) => Ok(Some(native)),
            Ok(InstalledLaunch::Missing | InstalledLaunch::UnsupportedHost) => Ok(None),
            Ok(InstalledLaunch::Unknown(_)) | Err(_) => Err(ConversationError::Unavailable),
        }
    }

    fn managed_provider(
        &self,
        agent: AgentId,
        model: &str,
        mode: ApprovalMode,
    ) -> Result<Option<agent::build::ProviderComposition>, ConversationError> {
        let Some(native) = self.managed_native(agent)? else {
            return Ok(None);
        };
        agent::provider_for_managed_adapter(
            agent,
            &self.config,
            model,
            mode,
            &self.provider,
            native,
        )
        .map(Some)
        .map_err(|error| {
            tracing::warn!(%error, "managed agent adapter could not be composed");
            ConversationError::ModelUnavailable
        })
    }

    fn managed_evidence(&self, agent: AgentId) -> AgentProbeEvidence {
        self.managed_evidence_with(agent, |probe| probe.evidence(agent))
    }

    fn managed_evidence_with(
        &self,
        agent: AgentId,
        observe: impl FnOnce(LocalAgentProbe) -> Option<AgentProbeEvidence>,
    ) -> AgentProbeEvidence {
        let unavailable = || AgentProbeEvidence {
            installed: Err(ProbeFailure::Unanswered),
            authenticated: Some(Err(ProbeFailure::Unanswered)),
        };
        let Ok(mut pending) = self.probe_cleanup.lock() else {
            return unavailable();
        };
        if let Some(cleanup) = pending.get_mut(&agent) {
            match cleanup {
                ProbeCleanup::Release(guard) => {
                    if guard.release().is_err() {
                        return unavailable();
                    }
                }
                _ => return unavailable(),
            }
            pending.remove(&agent);
        }
        let native = match self.managed_native(agent) {
            Ok(Some(native)) => native,
            Ok(None) => {
                return AgentProbeEvidence {
                    installed: Ok(false),
                    authenticated: None,
                }
            }
            Err(_) => return unavailable(),
        };
        let Some(runtime) = self.config.runtime(agent) else {
            return unavailable();
        };
        let Ok(launch) = ManagedAdapter::new(agent, runtime, native) else {
            return unavailable();
        };
        let mut environment = agent::launch_environment(agent);
        environment.extend(launch.environment);
        let files = AgentLaunchFiles {
            command: launch.runtime.command.executable().to_owned(),
            paths: launch.runtime.paths(),
            environment,
        };
        // Readiness may run the adapter's account-status command. Its native
        // dependency has the same process-use admission as a conversation.
        let mut guard = match launch.runtime.command.admit() {
            Ok(guard) => guard,
            Err(failure) => {
                if let Some(owner) = failure.into_parts().1 {
                    let mut guard = owner.into_guard();
                    if guard.release().is_err() {
                        pending.insert(agent, ProbeCleanup::Release(guard));
                    }
                }
                return unavailable();
            }
        };
        let evidence = observe(
            self.fixed_probe
                .with_launch_files(HashMap::from([(agent, files)])),
        )
        .unwrap_or_else(unavailable);
        if matches!(
            evidence.authenticated,
            Some(Err(ProbeFailure::CleanupUnconfirmed))
        ) {
            // Preserve one exact owner, without admitting more probes for this
            // agent. Durable use evidence continues to block reclamation after
            // restart; uncertain process cleanup cannot be guessed away.
            pending.insert(agent, ProbeCleanup::UnknownProcess { _guard: guard });
            return evidence;
        }
        if guard.release().is_err() {
            pending.insert(agent, ProbeCleanup::Release(guard));
            return unavailable();
        }
        evidence
    }

    async fn resolve_managed(
        &self,
        agent: AgentId,
        model: String,
        mode: ApprovalMode,
    ) -> Result<Option<ConversationAgent>, ConversationError> {
        tokio::time::timeout(RESOLUTION_DEADLINE, async {
            let permit = self
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ConversationError::Unavailable)?;
            let source = self.clone();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let Some(built) = source.managed_provider(agent, &model, mode)? else {
                    return Ok(None);
                };
                Ok(Some(ConversationAgent {
                    provider: built.provider,
                    execution_audit: built.execution_audit,
                    reserved_output_tokens: source
                        .config
                        .runtime(agent)
                        .expect("configured managed adapter")
                        .output_tokens,
                    readiness: None,
                }))
            })
            .await
            .map_err(|_| ConversationError::Unavailable)?
        })
        .await
        .map_err(|_| ConversationError::Unavailable)?
    }

    fn observe_opencode(&self) -> Observation {
        let profile = self
            .opencode
            .configured()
            .expect("unconfigured profiles return before observation");
        let launch = self.opencode_runtime(profile);
        let credential = self.opencode_credential(profile);
        let installed = match &launch {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(failure) => Err(*failure),
        };
        let authenticated = match &credential {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(_) => Err(ProbeFailure::Unanswered),
        };
        let evidence = AgentProbeEvidence {
            installed,
            authenticated: Some(authenticated),
        };
        let mut configured = true;
        let provider = match (launch, credential) {
            (Ok(Some(runtime)), Ok(Some(credential))) => {
                let environment = opencode_environment(credential);
                match agent::provider_for(
                    AgentId::Opencode,
                    &self.config,
                    &runtime,
                    &self.provider,
                    environment,
                    profile.validated(),
                ) {
                    Ok(provider) => Ok(Some(provider)),
                    Err(error) => {
                        tracing::error!(%error, "current OpenCode provider could not be built");
                        configured = false;
                        Err(crate::conversation::application::ConversationError::AgentNotConfigured)
                    }
                }
            }
            (Ok(None), _) | (_, Ok(None)) => Ok(None),
            (Err(_), _) | (_, Err(_)) => {
                Err(crate::conversation::application::ConversationError::Unavailable)
            }
        };
        Observation {
            evidence: configured.then_some(evidence),
            provider,
        }
    }

    /// How the current OpenCode generation deletes its own record of a
    /// session: `None` when it is not installed or has no credential now.
    fn opencode_session_eraser(
        &self,
    ) -> Result<Option<Arc<dyn ProviderSessionEraser>>, ConversationError> {
        let Some(profile) = self.opencode.configured() else {
            return Ok(None);
        };
        match (
            self.opencode_runtime(profile),
            self.opencode_credential(profile),
        ) {
            (Ok(Some(runtime)), Ok(Some(credential))) => agent::session_eraser_for(
                AgentId::Opencode,
                &self.config,
                &runtime,
                &self.provider,
                opencode_environment(credential),
                profile.validated(),
            )
            .map(Some)
            .map_err(|error| {
                tracing::error!(%error, "current OpenCode binding could not be built to delete a session");
                ConversationError::AgentNotConfigured
            }),
            (Ok(None), _) | (_, Ok(None)) => Ok(None),
            (Err(_), _) | (_, Err(_)) => Err(ConversationError::Unavailable),
        }
    }

    fn opencode_runtime(
        &self,
        profile: &OpenCodeProfile,
    ) -> Result<Option<AgentRuntime>, ProbeFailure> {
        if profile.managed() {
            return match installed_launch(AgentId::Opencode, &self.host, self.store.as_ref()) {
                Ok(InstalledLaunch::Ready(command)) => Ok(profile.managed_runtime(command)),
                Ok(InstalledLaunch::Missing) => Ok(None),
                Ok(InstalledLaunch::UnsupportedHost) => {
                    tracing::error!("configured managed OpenCode host became unsupported");
                    Err(ProbeFailure::Unanswered)
                }
                Ok(InstalledLaunch::Unknown(failure)) => {
                    tracing::warn!(%failure, "managed OpenCode installation state is unknown");
                    Err(ProbeFailure::Unanswered)
                }
                Err(error) => {
                    tracing::error!(%error, "managed OpenCode release could not be resolved");
                    Err(ProbeFailure::Unanswered)
                }
            };
        }
        let runtime = profile
            .explicit_runtime()
            .expect("standalone profile has an explicit launch");
        if !path_is_file(runtime.command.executable())? {
            return Ok(None);
        }
        for path in runtime.paths() {
            if !path_exists(&path)? {
                return Ok(None);
            }
        }
        Ok(Some(runtime))
    }

    fn opencode_credential(
        &self,
        profile: &OpenCodeProfile,
    ) -> Result<Option<OsString>, ProbeFailure> {
        match profile.credential() {
            OpenCodeCredentialMode::ScopedStore => self
                .credentials
                .read(AgentId::Opencode)
                .map_err(|_| ProbeFailure::Unanswered)
                .and_then(|credential| match credential {
                    Some(value) if value.kind() == AgentCredentialKind::ApiKey => {
                        Ok(Some(OsString::from(value.expose())))
                    }
                    Some(_) => Err(ProbeFailure::Unanswered),
                    None => Ok(None),
                }),
            OpenCodeCredentialMode::CapturedEnvironment(credential) => {
                captured_credential_environment(credential)
            }
        }
    }

    async fn resolve_opencode(
        &self,
    ) -> Result<Option<ConversationAgent>, crate::conversation::application::ConversationError>
    {
        loop {
            let permit =
                self.slots.clone().acquire_owned().await.map_err(|_| {
                    crate::conversation::application::ConversationError::Unavailable
                })?;
            let source = self.clone();
            let observation = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                source.observe_opencode().provider
            })
            .await
            .map_err(|_| crate::conversation::application::ConversationError::Unavailable)??;
            let Some(mut agent) = observation else {
                return Ok(None);
            };
            match self.warm_up.admit(&agent)? {
                CurrentWarmUpAdmission::Prepared(prepared) => {
                    agent.readiness = Some(Arc::new(prepared));
                    return Ok(Some(agent));
                }
                CurrentWarmUpAdmission::Reobserve(wait) => {
                    // `agent` owns this observation's credential-bearing provider.
                    // Drop it before waiting, then rebuild every external fact.
                    drop(agent);
                    wait.released().await;
                }
            }
        }
    }

    /// Whether the OpenCode warm-up may still hold a provider process: it is
    /// still being resolved, which ends in a launch, or the lane's active run
    /// may hold one. Counted from the moment it is scheduled, so there is no
    /// instant between "nothing yet" and a launch (ADR 221).
    pub(super) fn warm_up_may_hold_resources(&self) -> bool {
        self.warm_up_resolving.load(Ordering::SeqCst) > 0 || self.warm_up.may_hold_resources()
    }

    /// Start proactive preparation after the listener is accepting requests.
    /// Missing installation or credentials simply means there is nothing to
    /// prepare; a later cold conversation observes again and can start it.
    pub(super) fn start_warm_up(self: &Arc<Self>) {
        let source = self.clone();
        self.warm_up_resolving.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            // Released only once resolution has returned, by which time any
            // launch it made is the lane's to answer for.
            struct Resolving(Arc<CurrentAgentResolver>);
            impl Drop for Resolving {
                fn drop(&mut self) {
                    self.0.warm_up_resolving.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _resolving = Resolving(source.clone());
            let result = tokio::time::timeout(RESOLUTION_DEADLINE, source.resolve_opencode()).await;
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(%error, "OpenCode warm-up could not be resolved"),
                Err(_) => tracing::warn!("OpenCode warm-up resolution timed out"),
            }
        });
    }
}

impl AgentProbe for CurrentAgentResolver {
    fn evidence(&self, agent: AgentId) -> Option<AgentProbeEvidence> {
        if self.managed_adapters.contains(&agent) {
            match supports_installed_launch(agent, &self.host) {
                Ok(false) => return None,
                Err(_) => return Some(unknown_probe()),
                Ok(true) => {}
            }
            let Ok(_permit) = self.slots.clone().try_acquire_owned() else {
                return Some(unknown_probe());
            };
            return Some(self.managed_evidence(agent));
        }
        if agent != AgentId::Opencode {
            return self.fixed_probe.evidence(agent);
        }
        self.opencode.configured()?;
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return Some(AgentProbeEvidence {
                installed: Err(ProbeFailure::Unanswered),
                authenticated: Some(Err(ProbeFailure::Unanswered)),
            });
        };
        let observation = self.observe_opencode().evidence;
        drop(permit);
        observation
    }
}

impl ConversationAgentSource for CurrentAgentResolver {
    fn resolve(&self, agent: AgentId) -> ConversationAgentFuture<'_> {
        if self.managed_adapters.contains(&agent) {
            let model = self
                .config
                .runtime(agent)
                .expect("configured managed adapter")
                .model
                .clone();
            return Box::pin(self.resolve_managed(agent, model, ApprovalMode::Ask));
        }
        if agent != AgentId::Opencode {
            let configured = self.fixed.get(&agent).cloned();
            return Box::pin(async move { Ok(configured) });
        }
        if self.opencode.configured().is_none() {
            return Box::pin(async { Ok(None) });
        }
        let source = self.clone();
        Box::pin(async move {
            tokio::time::timeout(RESOLUTION_DEADLINE, source.resolve_opencode())
                .await
                .map_err(|_| crate::conversation::application::ConversationError::Unavailable)?
        })
    }

    fn resolve_for<'a>(
        &'a self,
        agent: AgentId,
        model: &'a str,
        mode: ConversationApprovalMode,
    ) -> ConversationAgentFuture<'a> {
        let selected = match mode {
            ConversationApprovalMode::Ask => ApprovalMode::Ask,
            ConversationApprovalMode::Auto => ApprovalMode::Auto,
            ConversationApprovalMode::Full => ApprovalMode::Full,
        };
        if agent == AgentId::Opencode {
            if mode != ConversationApprovalMode::Ask {
                return Box::pin(async { Err(ConversationError::ApprovalModeUnavailable) });
            }
            return Box::pin(async move {
                let configured = self.resolve(agent).await?;
                match configured {
                    Some(value) if value.provider.identity().model_id() != model => {
                        Err(ConversationError::ModelUnavailable)
                    }
                    other => Ok(other),
                }
            });
        }
        let managed = self.managed_adapters.contains(&agent);
        if !managed && !self.fixed.contains_key(&agent) {
            return Box::pin(async { Ok(None) });
        }
        let offered = match agent {
            AgentId::Claude => ClaudeAcpProvider::approval_modes(model),
            AgentId::Codex => CodexAcpProvider::approval_modes(model),
            AgentId::Opencode => unreachable!(),
        };
        if !offered.iter().any(|choice| choice.id == selected) {
            return Box::pin(async { Err(ConversationError::ApprovalModeUnavailable) });
        }
        if managed {
            return Box::pin(self.resolve_managed(agent, model.to_owned(), selected));
        }
        let default = self.fixed.get(&agent).expect("configured fixed agent");
        if mode == ConversationApprovalMode::Ask && default.provider.identity().model_id() == model
        {
            let configured = default.clone();
            return Box::pin(async move { Ok(Some(configured)) });
        }
        let source = self.clone();
        let model = model.to_owned();
        let readiness = default.readiness.clone();
        Box::pin(async move {
            let mut configured = tokio::task::spawn_blocking(move || {
                agent::provider_for_fixed(agent, &source.config, &model, selected, &source.provider)
            })
            .await
            .map_err(|_| ConversationError::Unavailable)?
            .map_err(|error| {
                tracing::warn!(%error, "conversation model could not be composed");
                ConversationError::ModelUnavailable
            })?;
            configured.readiness = readiness;
            Ok(Some(configured))
        })
    }
}

/// OpenCode's current generation, asked to delete its own record of a session.
///
/// Built from a fresh observation on every ask, in the resolver's one blocking
/// lane, as a cold conversation's provider is: not installed, or with no
/// credential now, is [`ConversationError::AgentNotConfigured`] — a known agent
/// not built this run, whose deletion stays unfinished until it is. Every
/// binding it launched is kept until it has settled.
pub(super) struct CurrentAgentEraser {
    agent: AgentId,
    resolver: Arc<CurrentAgentResolver>,
    launched: crate::conversation::infrastructure::LaunchedDeletions<dyn ProviderSessionEraser>,
}

impl CurrentAgentEraser {
    pub(super) fn new(resolver: Arc<CurrentAgentResolver>, agent: AgentId) -> Self {
        Self {
            agent,
            resolver,
            launched: crate::conversation::infrastructure::LaunchedDeletions::default(),
        }
    }
}

impl ProviderSessionEraser for CurrentAgentEraser {
    fn erase(
        &self,
        session: nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId,
    ) -> ConversationFuture<'_, ProviderSessionErasure> {
        Box::pin(async move {
            let agent = self.agent;
            let resolver = self.resolver.clone();
            let permit = resolver
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ConversationError::Unavailable)?;
            let eraser = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                if agent == AgentId::Opencode {
                    resolver.opencode_session_eraser()
                } else {
                    let runtime = resolver
                        .config
                        .runtime(agent)
                        .ok_or(ConversationError::AgentNotConfigured)?;
                    resolver
                        .managed_provider(agent, &runtime.model, ApprovalMode::Ask)
                        .map(|built| built.map(|value| value.session_eraser))
                }
            })
            .await
            .map_err(|_| ConversationError::Unavailable)??
            .ok_or(ConversationError::AgentNotConfigured)?;
            self.launched.push(eraser.clone());
            eraser.erase(session).await
        })
    }
    fn settled(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.launched.settled())
    }
    fn cleanup_outstanding(&self) -> bool {
        self.launched.cleanup_outstanding()
    }
}

/// The environment the current OpenCode generation is launched with.
fn opencode_environment(credential: OsString) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([(OsString::from("OPENCODE_API_KEY"), credential)])
}

fn path_is_file(path: &Path) -> Result<bool, ProbeFailure> {
    match path.metadata() {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ProbeFailure::Unanswered),
    }
}

fn path_exists(path: &Path) -> Result<bool, ProbeFailure> {
    match path.metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ProbeFailure::Unanswered),
    }
}

#[cfg(test)]
#[path = "../../tests/composition/current_agent.rs"]
mod tests;

/// Only confirmed process cleanup permits retrying durable release.
enum ProbeCleanup {
    Release(Box<dyn ExecutableUseGuard>),
    UnknownProcess { _guard: Box<dyn ExecutableUseGuard> },
}

fn unknown_probe() -> AgentProbeEvidence {
    AgentProbeEvidence {
        installed: Err(ProbeFailure::Unanswered),
        authenticated: Some(Err(ProbeFailure::Unanswered)),
    }
}
