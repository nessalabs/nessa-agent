//! Static OpenCode policy shared by selection, readiness, and provider creation.
//!
//! ```text
//! desktop/standalone inputs ─▶ EffectiveOpenCodeProfile
//!                                  ├─▶ configured/default + image limits
//!                                  └─▶ fresh launch/credential observation
//! ```
//!
//! Arrows mean composition decisions. The profile contains no managed launch:
//! each readiness or cold-open call resolves the current exact-pin snapshot.

use std::os::unix::ffi::OsStrExt;
use std::{ffi::OsString, path::PathBuf};

use nessa_sdk::application::agent_execution::providers::ExecutableUseSnapshot;

use super::{
    agent::{self, AgentRuntime, AgentsConfig, ValidatedOpenCodePolicy},
    installed_launch::{installed_arguments, supports_installed_launch},
};
use crate::{
    agent_install::domain::HostPlatform,
    agents::{
        application::{AgentCredential, AgentCredentialKind, ProbeFailure},
        domain::AgentId,
    },
};

const DEFAULT_MODEL: &str = "opencode/minimax-m3";

/// The one static answer composition uses for every OpenCode decision.
pub(super) enum EffectiveOpenCodeProfile {
    Configured(Box<OpenCodeProfile>),
    UnsupportedHost,
    InvalidPolicy(String),
    Absent,
}

pub(super) struct OpenCodeProfile {
    launch: OpenCodeLaunchPolicy,
    runtime: OpenCodeRuntimePolicy,
    credential: OpenCodeCredentialMode,
    validated: ValidatedOpenCodePolicy,
}

enum OpenCodeLaunchPolicy {
    ManagedCurrentPin {
        arguments: Vec<String>,
    },
    Explicit {
        command: ExecutableUseSnapshot,
        arguments: Vec<String>,
    },
}

#[derive(Clone)]
struct OpenCodeRuntimePolicy {
    model: String,
    tools_enabled: bool,
    context_tokens: u32,
    output_tokens: u32,
}

pub(super) enum OpenCodeCredentialMode {
    ScopedStore,
    CapturedEnvironment(CapturedOpenCodeCredential),
}

pub(super) enum CapturedOpenCodeCredential {
    Missing,
    ApiKey(AgentCredential),
    Invalid,
}

impl EffectiveOpenCodeProfile {
    pub(super) fn decide(
        config: &AgentsConfig,
        packaged: bool,
        host: &HostPlatform,
        captured_environment: Option<OsString>,
    ) -> Self {
        if packaged {
            return match supports_installed_launch(AgentId::Opencode, host) {
                Ok(false) => Self::UnsupportedHost,
                Err(error) => Self::InvalidPolicy(error.to_string()),
                Ok(true) => Self::packaged(config),
            };
        }
        Self::standalone(config, captured_environment)
    }

    fn packaged(config: &AgentsConfig) -> Self {
        let arguments = installed_arguments(AgentId::Opencode);
        let runtime = match config.runtime(AgentId::Opencode) {
            Some(explicit) if explicit.args != arguments => {
                return Self::InvalidPolicy(
                    "managed OpenCode arguments must be exactly [\"acp\"]".into(),
                );
            }
            Some(explicit) => OpenCodeRuntimePolicy::from(explicit),
            None => OpenCodeRuntimePolicy {
                model: DEFAULT_MODEL.into(),
                tools_enabled: true,
                context_tokens: 100_000,
                output_tokens: 4096,
            },
        };
        Self::validated(
            config,
            OpenCodeLaunchPolicy::ManagedCurrentPin { arguments },
            runtime,
            OpenCodeCredentialMode::ScopedStore,
        )
    }

    fn standalone(config: &AgentsConfig, captured_environment: Option<OsString>) -> Self {
        let Some(explicit) = config.runtime(AgentId::Opencode) else {
            return Self::Absent;
        };
        let credential = match captured_environment {
            None => CapturedOpenCodeCredential::Missing,
            Some(value) => match environment_credential(value) {
                Ok(credential) => CapturedOpenCodeCredential::ApiKey(credential),
                Err(()) => CapturedOpenCodeCredential::Invalid,
            },
        };
        Self::validated(
            config,
            OpenCodeLaunchPolicy::Explicit {
                command: explicit.command.clone(),
                arguments: explicit.args.clone(),
            },
            OpenCodeRuntimePolicy::from(explicit),
            OpenCodeCredentialMode::CapturedEnvironment(credential),
        )
    }

    fn validated(
        config: &AgentsConfig,
        launch: OpenCodeLaunchPolicy,
        runtime: OpenCodeRuntimePolicy,
        credential: OpenCodeCredentialMode,
    ) -> Self {
        let validation = runtime.runtime(
            ExecutableUseSnapshot::unmanaged(PathBuf::from("/nessa-opencode-policy")),
            launch.arguments().to_vec(),
        );
        match agent::validate_opencode_policy(config, &validation) {
            Ok(validated) => Self::Configured(Box::new(OpenCodeProfile {
                launch,
                runtime,
                credential,
                validated,
            })),
            Err(error) => Self::InvalidPolicy(error.to_string()),
        }
    }

    pub(super) fn configured(&self) -> Option<&OpenCodeProfile> {
        match self {
            Self::Configured(profile) => Some(profile),
            Self::UnsupportedHost | Self::InvalidPolicy(_) | Self::Absent => None,
        }
    }

    pub(super) fn invalid_reason(&self) -> Option<&str> {
        match self {
            Self::InvalidPolicy(reason) => Some(reason),
            Self::Configured(_) | Self::UnsupportedHost | Self::Absent => None,
        }
    }
}

impl OpenCodeProfile {
    pub(super) fn validated(&self) -> &ValidatedOpenCodePolicy {
        &self.validated
    }

    pub(super) fn credential(&self) -> &OpenCodeCredentialMode {
        &self.credential
    }

    pub(super) fn managed(&self) -> bool {
        matches!(self.launch, OpenCodeLaunchPolicy::ManagedCurrentPin { .. })
    }

    pub(super) fn managed_runtime(&self, command: ExecutableUseSnapshot) -> Option<AgentRuntime> {
        match &self.launch {
            OpenCodeLaunchPolicy::ManagedCurrentPin { arguments } => {
                Some(self.runtime.runtime(command, arguments.clone()))
            }
            OpenCodeLaunchPolicy::Explicit { .. } => None,
        }
    }

    pub(super) fn explicit_runtime(&self) -> Option<AgentRuntime> {
        match &self.launch {
            OpenCodeLaunchPolicy::Explicit { command, arguments } => {
                Some(self.runtime.runtime(command.clone(), arguments.clone()))
            }
            OpenCodeLaunchPolicy::ManagedCurrentPin { .. } => None,
        }
    }
}

impl OpenCodeLaunchPolicy {
    fn arguments(&self) -> &[String] {
        match self {
            Self::ManagedCurrentPin { arguments } | Self::Explicit { arguments, .. } => arguments,
        }
    }
}

impl OpenCodeRuntimePolicy {
    fn runtime(&self, command: ExecutableUseSnapshot, args: Vec<String>) -> AgentRuntime {
        AgentRuntime {
            command,
            args,
            model: self.model.clone(),
            tools_enabled: self.tools_enabled,
            context_tokens: self.context_tokens,
            output_tokens: self.output_tokens,
        }
    }
}

impl From<&AgentRuntime> for OpenCodeRuntimePolicy {
    fn from(runtime: &AgentRuntime) -> Self {
        Self {
            model: runtime.model.clone(),
            tools_enabled: runtime.tools_enabled,
            context_tokens: runtime.context_tokens,
            output_tokens: runtime.output_tokens,
        }
    }
}

fn environment_credential(value: OsString) -> Result<AgentCredential, ()> {
    AgentCredential::new(AgentCredentialKind::ApiKey, os_bytes(value)).map_err(|_| ())
}

fn os_bytes(value: OsString) -> Vec<u8> {
    value.as_os_str().as_bytes().to_vec()
}

pub(super) fn captured_credential_environment(
    credential: &CapturedOpenCodeCredential,
) -> Result<Option<OsString>, ProbeFailure> {
    match credential {
        CapturedOpenCodeCredential::Missing => Ok(None),
        CapturedOpenCodeCredential::ApiKey(credential) => {
            Ok(Some(OsString::from(credential.expose())))
        }
        CapturedOpenCodeCredential::Invalid => Err(ProbeFailure::Unanswered),
    }
}
