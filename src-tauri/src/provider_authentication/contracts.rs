use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LoginFailure {
    UntrustedCaller,
    Unavailable,
    LaunchFailed,
}

/// Outside-process login launch, supplied by host composition.
pub trait ProviderLogin: Send + Sync {
    /// Whether this host implements a provider login launcher.
    fn available(&self) -> bool;
    fn open(&self, provider: Provider) -> Result<(), LoginFailure>;
}
