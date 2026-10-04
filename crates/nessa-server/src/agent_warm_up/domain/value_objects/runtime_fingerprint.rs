use std::fmt;

/// Identity of the runtime the gateway would launch.
///
/// Two launches share a fingerprint when they would run the same provider, the
/// same model, and the same configuration. The configuration part is the
/// provider's own credential-free identity; for the ACP providers that is the
/// SDK's restoration fingerprint, whose inputs are listed on `fingerprint` in
/// the SDK's `acp/sessions/identity.rs`. This is the one statement of what a
/// warm-up covers; the gateway's composition links here.
///
/// A warm-up exists for the agent runtime's first-execution scan, which is
/// paid per file. An install, or an update that changes the staged runtime
/// tree, stages the runtime under a new directory named by that tree's content
/// fingerprint, and the agent runtime's executable and entry are launched from
/// it, so their paths, and with them this value, change with every such
/// update, which in practice is every release.
///
/// Because the configuration part is the SDK's restoration fingerprint, the
/// release that dropped the MCP servers from that fingerprint (#391) re-runs
/// the warm-up once; that is one background launch, and harmless. What else
/// that release changed is in `docs/design/mcp-connections.md`, "MCP servers
/// and the restoration identity".
///
/// A warm-up opens a real session, so it may also launch the MCP servers
/// configured at that time. The server list is not among the inputs, though,
/// so changing it does not re-warm: a server added later pays its first
/// launch, and any first-run scan it needs, when a conversation first uses
/// it. The bundled `nessa` server is covered only because it sits in the same
/// versioned directory as the agent runtime, whose paths are hashed: an update
/// that moves that directory moves the runtime, and the re-warm that follows
/// may launch the new `nessa` with it.
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

/// Why a warm-up happened.
///
/// There is one reason and it is not a person: the gateway launched a runtime
/// it had never launched before. The audit gate wants that carried from the
/// application through its own port, so this is a value the service supplies
/// rather than a label the writer adds on the way to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmUpCause {
    /// The gateway prepared a configured runtime nobody had asked it to.
    AutomaticPreparation,
}
impl WarmUpCause {
    /// Stable lowercase identifier for audit records.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutomaticPreparation => "automatic_runtime_warm_up",
        }
    }
}
impl fmt::Display for WarmUpCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "../../../../tests/agent_warm_up/runtime_fingerprint.rs"]
mod tests;
