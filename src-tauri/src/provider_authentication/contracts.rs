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
    /// The sign-in ended, was abandoned or timed out without a token.
    NotCompleted,
}

/// Outside-process login launch, supplied by host composition.
pub trait ProviderLogin: Send + Sync {
    /// Whether this host implements a provider login launcher.
    fn available(&self) -> bool;
    fn open(&self, provider: Provider) -> Result<(), LoginFailure>;
    /// Whether this host can mint a Claude sign-in for another machine.
    fn mints_claude_sign_in(&self) -> bool {
        false
    }
    /// Run Claude's own `setup-token` in the background: the person signs in
    /// in their browser, and the long-lived token it prints is answered.
    /// Blocks until then.
    fn mint_claude_sign_in(&self) -> Result<String, LoginFailure> {
        Err(LoginFailure::Unavailable)
    }
}
