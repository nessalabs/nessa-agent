use std::fmt;

/// Identity of the runtime the gateway would launch.
///
/// Two launches share a fingerprint when they would run the same provider, the
/// same model, and the same configuration. The configuration part is the
/// provider's own credential-free identity — for the ACP provider that covers
/// the executable, its arguments, the environment it is given, the workspace,
/// whether tools are enabled, every MCP server binary it will start, the token
/// limits, the permission policy and the system prompt — so anything that
/// changes which files are executed changes this value.
///
/// That matters because a first-execution scan is paid per file. An install or
/// an update stages the runtime under a new directory and replaces the MCP
/// binaries beside it, and every one of those is inside the provider's
/// configuration identity.
///
/// The model is part of it too, although changing a model scans nothing: a
/// warm-up also proves the configuration establishes a session, and that is
/// worth redoing when the session parameters change. The cost of being wrong
/// in this direction is one background launch.
///
/// This is a pure comparison of what was configured. It reads nothing from disk
/// and makes no claim that the files exist or are unchanged in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeFingerprint {
    provider: String,
    model: String,
    configuration: String,
}

/// Why a proposed fingerprint does not identify a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFingerprintError {
    /// The provider or model was empty, so it names nothing. An empty
    /// configuration is allowed: a provider may have no settings to describe.
    Empty,
    /// A part exceeded the retained length bound.
    TooLong,
}
impl fmt::Display for RuntimeFingerprintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "runtime fingerprint provider and model must not be empty",
            Self::TooLong => "runtime fingerprint parts must be at most 4096 bytes",
        })
    }
}
impl std::error::Error for RuntimeFingerprintError {}

const MAX_PART_BYTES: usize = 4096;

impl RuntimeFingerprint {
    /// Identify a runtime by the provider implementation, the exact model, and
    /// the provider's credential-free configuration identity.
    ///
    /// # Errors
    /// Rejects an empty provider or model, and any part longer than 4096 bytes;
    /// a fingerprint is retained and compared, so it is bounded like any other
    /// stored identity.
    pub fn new(
        provider: &str,
        model: &str,
        configuration: &str,
    ) -> Result<Self, RuntimeFingerprintError> {
        if provider.is_empty() || model.is_empty() {
            return Err(RuntimeFingerprintError::Empty);
        }
        for part in [provider, model, configuration] {
            if part.len() > MAX_PART_BYTES {
                return Err(RuntimeFingerprintError::TooLong);
            }
        }
        Ok(Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
            configuration: configuration.to_owned(),
        })
    }
    /// Provider implementation that would be launched.
    pub fn provider(&self) -> &str {
        &self.provider
    }
    /// Exact model it would be asked for.
    pub fn model(&self) -> &str {
        &self.model
    }
    /// The provider's credential-free configuration identity.
    pub fn configuration(&self) -> &str {
        &self.configuration
    }
}

/// Whether this runtime has completed a warm-up.
///
/// The two states are the before and after of one transition, which is what an
/// audit record of that transition has to carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmUpState {
    /// No completed warm-up is recorded for this runtime.
    Cold,
    /// This runtime has been through a full open and close at least once.
    Warmed,
}
impl WarmUpState {
    /// Stable lowercase identifier for audit records and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Warmed => "warmed",
        }
    }
}
impl fmt::Display for WarmUpState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_warm_up/runtime_fingerprint.rs"]
mod tests;
