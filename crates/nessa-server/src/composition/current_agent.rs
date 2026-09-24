//! Current agent observations for readiness and cold conversation opening.
//!
//! Composition owns the relationship between the managed runtime store,
//! stage-scoped credentials, and provider construction. Readiness and a cold
//! slot each take a fresh observation; a live conversation keeps the provider
//! generation it already owns.
//!
//! ```text
//! readiness ─┐
//!            ├─▶ CurrentAgentResolver ─▶ runtime store + credential source
//! cold slot ─┘                         └▶ owned provider generation
//! ```
//!
//! Arrows are calls. OpenCode's blocking observation has one bounded lane. The
//! blocking task owns its permit through actual completion, even when its
//! awaiting caller times out or is dropped.

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
    desktop,
    installed_launch::{installed_launch, InstalledLaunch},
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
    conversation::application::{
        ConversationAgent, ConversationAgentFuture, ConversationAgentSource,
    },
    core::RunError,
};

const RESOLUTION_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct CurrentAgentResolver {
    fixed: HashMap<AgentId, ConversationAgent>,
    fixed_probe: Arc<LocalAgentProbe>,
    config: AgentsConfig,
    managed_opencode: bool,
    store: Arc<dyn RuntimeStore>,
    host: HostPlatform,
    credentials: Arc<dyn AgentCredentialSource>,
    provider: ProviderDependencies,
    slots: Arc<Semaphore>,
}

pub(super) struct CurrentAgentResolverInput {
    pub(super) fixed: HashMap<AgentId, ConversationAgent>,
    pub(super) fixed_probe: LocalAgentProbe,
    pub(super) config: AgentsConfig,
    pub(super) managed_opencode: bool,
    pub(super) store: Arc<dyn RuntimeStore>,
    pub(super) host: HostPlatform,
    pub(super) credentials: Arc<dyn AgentCredentialSource>,
    pub(super) provider_directory: PathBuf,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) images: Arc<dyn UserImageSource>,
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
            managed_opencode: input.managed_opencode,
            store: input.store,
            host: input.host,
            credentials: input.credentials.clone(),
            provider: ProviderDependencies {
                directory: input.provider_directory,
                clock: input.clock,
                images: input.images,
                credentials: input.credentials,
            },
            slots: Arc::new(Semaphore::new(1)),
        }
    }

    pub(super) fn configured(&self) -> HashSet<AgentId> {
        let mut configured: HashSet<_> = self.fixed.keys().copied().collect();
        if self.managed_opencode || self.config.runtime(AgentId::Opencode).is_some() {
            configured.insert(AgentId::Opencode);
        }
        configured
    }

    pub(super) fn default_agent(&self) -> Result<AgentId, RunError> {
        self.config.selected_from(&self.configured())
    }

    fn observe_opencode(&self) -> Observation {
        let launch = self.opencode_runtime();
        let credential = self.credentials.read(AgentId::Opencode);
        let installed = match &launch {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(failure) => Err(*failure),
        };
        let authenticated = match &credential {
            Ok(Some(value)) if value.kind() == AgentCredentialKind::ApiKey => Ok(true),
            Ok(None) => Ok(false),
            Ok(Some(_)) | Err(_) => Err(ProbeFailure::Unanswered),
        };
        let evidence = AgentProbeEvidence {
            installed,
            authenticated: Some(authenticated),
        };
        let mut configured = true;
        let provider = match (launch, credential) {
            (Ok(Some(runtime)), Ok(Some(credential)))
                if credential.kind() == AgentCredentialKind::ApiKey =>
            {
                let environment = BTreeMap::from([(
                    OsString::from("OPENCODE_API_KEY"),
                    OsString::from(credential.expose()),
                )]);
                match agent::provider_for(
                    AgentId::Opencode,
                    &self.config,
                    &runtime,
                    &self.provider,
                    environment,
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
            (Err(_), _) | (_, Err(_)) | (_, Ok(Some(_))) => {
                Err(crate::conversation::application::ConversationError::Unavailable)
            }
        };
        Observation {
            evidence: configured.then_some(evidence),
            provider,
        }
    }

    fn opencode_runtime(&self) -> Result<Option<AgentRuntime>, ProbeFailure> {
        if self.managed_opencode {
            return match installed_launch(AgentId::Opencode, &self.host, self.store.as_ref()) {
                Ok(InstalledLaunch::Ready(command)) => {
                    Ok(Some(desktop::managed_runtime(AgentId::Opencode, command)))
                }
                Ok(InstalledLaunch::Missing | InstalledLaunch::UnsupportedHost) => Ok(None),
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
        let Some(runtime) = self.config.runtime(AgentId::Opencode).cloned() else {
            return Ok(None);
        };
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
}

impl AgentProbe for CurrentAgentResolver {
    fn evidence(&self, agent: AgentId) -> Option<AgentProbeEvidence> {
        if agent != AgentId::Opencode {
            return self.fixed_probe.evidence(agent);
        }
        if !self.configured().contains(&agent) {
            return None;
        }
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
        let source = self.clone();
        Box::pin(async move {
            tokio::time::timeout(RESOLUTION_DEADLINE, async move {
                let permit = source.slots.clone().acquire_owned().await.map_err(|_| {
                    crate::conversation::application::ConversationError::Unavailable
                })?;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    source.observe_opencode().provider
                })
                .await
                .map_err(|_| crate::conversation::application::ConversationError::Unavailable)?
            })
            .await
            .map_err(|_| crate::conversation::application::ConversationError::Unavailable)?
        })
    }
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
