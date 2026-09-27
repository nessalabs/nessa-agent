use std::{error::Error, fmt};

/// Redacted command failures; protocol/provider text never becomes diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliError {
    NonLocal,
    CredentialUnavailable,
    InvalidCredential,
    Expired,
    InvalidLifetime,
    Unreachable,
    Transport,
    TimedOut,
    Protocol,
    Correlation,
    Unauthorized,
    Denied,
    Capacity,
    Rejected,
    Unhealthy,
    SecretUnavailable,
    ScopeMismatch,
}
impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NonLocal => "local gateway must use loopback",
            Self::CredentialUnavailable => "cannot read private CLI credential; run nessa auth init --local once, or provide --credential-file",
            Self::InvalidCredential => "invalid CLI credential",
            Self::InvalidLifetime => "token TTL must be positive and representable",
            Self::Expired => "CLI credential has expired",
            Self::Unreachable => "gateway unreachable; start nessa server",
            Self::Transport => "gateway connection ended or request delivery failed",
            Self::TimedOut => "gateway command timed out",
            Self::Protocol => "invalid or incompatible gateway protocol",
            Self::Correlation => "uncorrelated gateway response",
            Self::Unauthorized => "CLI credential was rejected",
            Self::Denied => "CLI credential is not authorized for this command",
            Self::Capacity => "gateway credential capacity reached",
            Self::Rejected => "gateway rejected the command",
            Self::Unhealthy => "gateway health probe failed",
            Self::SecretUnavailable => "token secret unavailable; inspect credentials before issuing again",
            Self::ScopeMismatch => "issued credential does not match requested browser access",
        })
    }
}
impl Error for CliError {}
