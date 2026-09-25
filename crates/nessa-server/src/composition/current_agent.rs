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
//! ([`CurrentOpenCodeEraser`]). The
//! blocking task owns its permit through actual completion, even when its
//! awaiting caller times out or is dropped. A different warm-up fingerprint
//! waits without retaining that observation, then resolves every external fact
//! again after the active run confirms physical release.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use nessa_auth::application::ports::Clock;
use nessa_sdk::application::agent_execution::providers::UserImageSource;
use tokio::sync::Semaphore;

use super::{
    agent::{self, AgentRuntime, AgentsConfig, ProviderDependencies},
    installed_launch::{installed_launch, InstalledLaunch},
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
        domain::AgentId,
        infrastructure::LocalAgentProbe,
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

const RESOLUTION_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct CurrentAgentResolver {
    fixed: HashMap<AgentId, ConversationAgent>,
    fixed_probe: Arc<LocalAgentProbe>,
    config: AgentsConfig,
    opencode: Arc<EffectiveOpenCodeProfile>,
    store: Arc<dyn RuntimeStore>,
    host: HostPlatform,
    credentials: Arc<dyn AgentCredentialSource>,
    provider: ProviderDependencies,
    warm_up: CurrentOpenCodeWarmUp,
    slots: Arc<Semaphore>,
}

pub(super) struct CurrentAgentResolverInput {
    pub(super) fixed: HashMap<AgentId, ConversationAgent>,
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
            slots: Arc::new(Semaphore::new(1)),
        }
    }

    pub(super) fn configured(&self) -> HashSet<AgentId> {
        let mut configured: HashSet<_> = self.fixed.keys().copied().collect();
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
                Arc::new(CurrentOpenCodeEraser::new(self.clone())),
            );
        }
    }

    pub(super) fn default_agent(&self) -> Result<AgentId, RunError> {
        self.config.selected_from(&self.configured())
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

    /// Start proactive preparation after the listener is accepting requests.
    /// Missing installation or credentials simply means there is nothing to
    /// prepare; a later cold conversation observes again and can start it.
    pub(super) fn start_warm_up(self: &Arc<Self>) {
        let source = self.clone();
        tokio::spawn(async move {
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
}

/// OpenCode's current generation, asked to delete its own record of a session.
///
/// Built from a fresh observation on every ask, in the resolver's one blocking
/// lane, as a cold conversation's provider is: not installed, or with no
/// credential now, is [`ConversationError::AgentNotConfigured`] — a known agent
/// not built this run, whose deletion stays unfinished until it is. Every
/// binding it launched is kept until it has settled.
pub(super) struct CurrentOpenCodeEraser {
    resolver: Arc<CurrentAgentResolver>,
    launched: std::sync::Mutex<Vec<Arc<dyn ProviderSessionEraser>>>,
}

impl CurrentOpenCodeEraser {
    pub(super) fn new(resolver: Arc<CurrentAgentResolver>) -> Self {
        Self {
            resolver,
            launched: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl ProviderSessionEraser for CurrentOpenCodeEraser {
    fn erase(
        &self,
        session: nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId,
    ) -> ConversationFuture<'_, ProviderSessionErasure> {
        Box::pin(async move {
            let resolver = self.resolver.clone();
            let permit = resolver
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ConversationError::Unavailable)?;
            let eraser = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                resolver.opencode_session_eraser()
            })
            .await
            .map_err(|_| ConversationError::Unavailable)??
            .ok_or(ConversationError::AgentNotConfigured)?;
            self.launched
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(eraser.clone());
            eraser.erase(session).await
        })
    }
    fn settled(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let launched = std::mem::take(
                &mut *self
                    .launched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            for eraser in launched {
                eraser.settled().await;
            }
        })
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
