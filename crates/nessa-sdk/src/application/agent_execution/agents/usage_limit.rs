//! A provider usage limit, kept as one typed failure.
//!
//! Timed exhaustion, an unknown reset, and an action the person must take do
//! not leave work owed. The resumption scan is what withholds the turns the
//! runner stopped behind this error. A temporary in-harness retry (`willRetry`)
//! is not this failure.

use super::error::ProviderDiagnostic;

/// Which harness reported the limit. OpenCode is not a variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageLimitProvider {
    /// Claude ACP 0.76.0 / Claude Agent SDK 0.3.257.
    Claude,
    /// Codex ACP 1.12.0.
    Codex,
}

/// Which window the harness named, when it named one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageLimitKind {
    /// Claude `five_hour`.
    FiveHour,
    /// Claude seven-day windows that are not model-specific.
    SevenDay,
    /// Claude `seven_day_opus` or `seven_day_sonnet`.
    ModelSpecific,
    /// Claude overage window.
    Overage,
    /// The harness did not name a window.
    Unspecified,
}

/// How the limit can end. The variant is the only place a reset instant lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageLimitRecovery {
    /// `resets_at_ms` is an absolute Unix instant in milliseconds.
    TimedReset,
    /// The account needs a person (billing, credits, an admin). No reset.
    UserAction,
    /// A usage limit with no trustworthy reset instant.
    Unknown,
}

/// One usage-limit failure. `resets_at_ms` is set only for [`UsageLimitRecovery::TimedReset`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageLimit {
    provider: UsageLimitProvider,
    kind: UsageLimitKind,
    recovery: UsageLimitRecovery,
    resets_at_ms: Option<u64>,
    diagnostic: Option<ProviderDiagnostic>,
}

impl UsageLimit {
    /// A rejected window with a parsed absolute reset.
    pub fn timed_reset(
        provider: UsageLimitProvider,
        kind: UsageLimitKind,
        resets_at_ms: u64,
        diagnostic: Option<ProviderDiagnostic>,
    ) -> Self {
        Self {
            provider,
            kind,
            recovery: UsageLimitRecovery::TimedReset,
            resets_at_ms: Some(resets_at_ms),
            diagnostic,
        }
    }

    /// Billing, credits, or an admin hold. A reset on the wire is not kept.
    pub fn user_action(
        provider: UsageLimitProvider,
        kind: UsageLimitKind,
        diagnostic: Option<ProviderDiagnostic>,
    ) -> Self {
        Self {
            provider,
            kind,
            recovery: UsageLimitRecovery::UserAction,
            resets_at_ms: None,
            diagnostic,
        }
    }

    /// The harness named a usage limit and no trustworthy reset.
    pub fn unknown(
        provider: UsageLimitProvider,
        kind: UsageLimitKind,
        diagnostic: Option<ProviderDiagnostic>,
    ) -> Self {
        Self {
            provider,
            kind,
            recovery: UsageLimitRecovery::Unknown,
            resets_at_ms: None,
            diagnostic,
        }
    }

    /// Restore a value the snapshot already checked.
    pub(crate) fn restore(
        provider: UsageLimitProvider,
        kind: UsageLimitKind,
        recovery: UsageLimitRecovery,
        resets_at_ms: Option<u64>,
        diagnostic: Option<ProviderDiagnostic>,
    ) -> Result<Self, &'static str> {
        let limit = match recovery {
            UsageLimitRecovery::TimedReset => {
                let Some(resets_at_ms) = resets_at_ms else {
                    return Err("a timed usage limit has no reset");
                };
                Self::timed_reset(provider, kind, resets_at_ms, diagnostic)
            }
            UsageLimitRecovery::UserAction => {
                if resets_at_ms.is_some() {
                    return Err("a usage limit that needs the person carries a reset");
                }
                Self::user_action(provider, kind, diagnostic)
            }
            UsageLimitRecovery::Unknown => {
                if resets_at_ms.is_some() {
                    return Err("an unknown usage limit carries a reset");
                }
                Self::unknown(provider, kind, diagnostic)
            }
        };
        Ok(limit)
    }

    /// Which harness reported it.
    pub fn provider(&self) -> UsageLimitProvider {
        self.provider
    }

    /// Which window the harness named.
    pub fn kind(&self) -> UsageLimitKind {
        self.kind
    }

    /// How the limit can end.
    pub fn recovery(&self) -> UsageLimitRecovery {
        self.recovery
    }

    /// Absolute Unix milliseconds, only when [`Self::recovery`] is a timed reset.
    pub fn resets_at_ms(&self) -> Option<u64> {
        self.resets_at_ms
    }

    /// Bounded provider prose. It does not choose the recovery.
    pub fn diagnostic(&self) -> Option<&ProviderDiagnostic> {
        self.diagnostic.as_ref()
    }
}
