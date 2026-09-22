#![deny(missing_docs)]

/// Whether a provider can carry a denial for a permission review it raised.
///
/// This says nothing about tools that do not pass through a permission review,
/// and it does not imply that Nessa has a configured pre-tool policy evaluator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PermissionDenialCapability {
    /// The binding has not established whether a denial can be delivered.
    #[default]
    Unknown,
    /// The binding cannot deliver a denial through a permission review.
    Unsupported,
    /// Nessa can select a rejecting option from a provider-raised review that offers one.
    SupportedForOfferedPermissionReviews,
}

/// Whether user-configured executable hooks are proven disabled in the provider.
///
/// Provider-owned builtin resource cleanup is outside this capability. A requested
/// setting and an absence of observed hook events are not proof of suppression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeHookSuppressionCapability {
    /// The effective provider configuration has not been verified.
    #[default]
    Unknown,
    /// This binding cannot suppress user-configured executable hooks.
    Unsupported,
    /// User-configured executable hooks are proven disabled for this provider context.
    SupportedForUserConfiguredHooks,
}

/// Whether provider compaction is reported with the invocation it affected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderCompactionReportingCapability {
    /// Delivery and correlation have not been verified.
    #[default]
    Unknown,
    /// The binding cannot report provider compaction.
    Unsupported,
    /// The binding reports completed compaction with stable invocation correlation.
    SupportedWithInvocationCorrelation,
}

/// Whether provider model switches are reported after their effective state is verified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderModelSwitchReportingCapability {
    /// Delivery or the reported phase has not been verified.
    #[default]
    Unknown,
    /// The binding cannot report provider model switches.
    Unsupported,
    /// The binding reports the validated model after a completed switch.
    SupportedAfterValidatedSwitch,
}

/// Whether a provider offers an explicit nonterminal defer outcome for a permission review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderPermissionDeferralCapability {
    /// The provider's explicit defer behavior has not been verified.
    #[default]
    Unknown,
    /// The provider has no explicit defer outcome that keeps the review unresolved.
    Unsupported,
    /// A defer outcome keeps the same review pending for one correlated later answer.
    SupportedWithNonterminalOutcome,
}

/// Whether an MCP server elicitation completes one correlated provider round trip.
///
/// This is provider forwarding evidence only. Nessa's incoming elicitation use case
/// is reported separately and cannot be enabled by this value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ElicitationForwardingCapability {
    /// The complete server-to-client-to-server path has not been verified.
    #[default]
    Unknown,
    /// The binding cannot forward and resolve MCP elicitation.
    Unsupported,
    /// The binding forwards a request and returns its correlated result.
    SupportedWithCorrelatedRoundTrip,
}

/// Provider facts that an adapter may publish after its connection is verified.
///
/// These facts contain no Nessa policy decision. [`ProviderSession`](super::ProviderSession)
/// resolves them into [`OperationCapabilities`], which keeps application-only features
/// unavailable until their common implementation exists.
///
/// # Examples
///
/// ```
/// use nessa_sdk::application::agent_execution::providers::{
///     PermissionDenialCapability, ProviderOperationCapabilities,
/// };
///
/// let provider = ProviderOperationCapabilities {
///     negotiated: true,
///     permission_denial: PermissionDenialCapability::SupportedForOfferedPermissionReviews,
///     ..ProviderOperationCapabilities::default()
/// };
/// assert!(provider.negotiated);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderOperationCapabilities {
    /// The other fields came from a completed, verified provider connection.
    /// False while opening or restoring means provider-derived facts are unknown.
    pub negotiated: bool,
    /// The provider can inject input into an identified active invocation.
    pub native_steering: bool,
    /// The provider supports restoring the same context after connection close.
    pub session_resume: bool,
    /// The connected agent accepts images and this binding can supply their bytes.
    pub image_input: bool,
    /// Delivery support for a rejecting permission option.
    pub permission_denial: PermissionDenialCapability,
    /// Effective suppression of user-configured provider hooks.
    pub native_hook_suppression: NativeHookSuppressionCapability,
    /// Correlated provider compaction reporting.
    pub compaction_reporting: ProviderCompactionReportingCapability,
    /// Validated provider model-switch reporting.
    pub model_switch_reporting: ProviderModelSwitchReportingCapability,
    /// An explicit nonterminal defer outcome for permission reviews.
    pub permission_deferral: ProviderPermissionDeferralCapability,
    /// Correlated provider forwarding of MCP elicitation.
    pub elicitation_forwarding: ElicitationForwardingCapability,
}

/// Whether a configured Nessa policy can deny a tool before execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreToolPolicyCapability {
    /// Coverage of the execution gate has not been established.
    Unknown,
    /// The configured pre-tool policy evaluator is not implemented.
    UnsupportedNotImplemented,
    /// Policy denial is enforced for calls held at a proven permission gate.
    SupportedAtPermissionGate,
}

/// Whether a Nessa policy can end its current invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyEndTurnCapability {
    /// Correlation and target fencing have not been established.
    Unknown,
    /// Policy integration for ending an invocation is not implemented.
    UnsupportedNotImplemented,
    /// A policy can stop its correlated current invocation.
    SupportedForCurrentInvocation,
}

/// Whether a Nessa policy can close its provider session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyCloseSessionCapability {
    /// Correlation and cleanup ownership have not been established.
    Unknown,
    /// Policy integration for closing a session is not implemented.
    UnsupportedNotImplemented,
    /// A policy can close its correlated session through the lifecycle owner.
    SupportedForSession,
}

/// Whether Nessa can receive and resolve an incoming elicitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncomingElicitationCapability {
    /// The application path has not been established.
    Unknown,
    /// The incoming elicitation use case is not implemented.
    UnsupportedNotImplemented,
    /// Nessa accepts one request and returns its correlated result.
    SupportedWithCorrelatedRoundTrip,
}

/// Whether Nessa reports provider compaction with invocation correlation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionReportingCapability {
    /// Reporting integration is not implemented in Nessa.
    UnsupportedNotImplemented,
    /// Nessa reports completed compaction with stable invocation correlation.
    SupportedWithInvocationCorrelation,
}

/// Whether Nessa reports a validated provider model switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSwitchReportingCapability {
    /// Reporting integration is not implemented in Nessa.
    UnsupportedNotImplemented,
    /// Nessa reports the validated model after a completed switch.
    SupportedAfterValidatedSwitch,
}

/// Whether Nessa can explicitly defer a permission review for a later answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDeferralCapability {
    /// Explicit defer integration is not implemented in Nessa.
    UnsupportedNotImplemented,
    /// A defer outcome keeps the review pending for one correlated later answer.
    SupportedWithNonterminalOutcome,
}

/// Effective operations available to an Agent.
///
/// This is a point-in-time support snapshot, not an admission permit. Provider
/// facts reset to unknown during restoration. Known application absences remain
/// unsupported because this application resolver, rather than an adapter, owns them.
/// Closing retains the last resolved snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationCapabilities {
    provider: ProviderOperationCapabilities,
    compaction_reporting: CompactionReportingCapability,
    model_switch_reporting: ModelSwitchReportingCapability,
    permission_deferral: PermissionDeferralCapability,
    pre_tool_policy: PreToolPolicyCapability,
    policy_end_turn: PolicyEndTurnCapability,
    policy_close_session: PolicyCloseSessionCapability,
    incoming_elicitation: IncomingElicitationCapability,
}

impl Default for OperationCapabilities {
    fn default() -> Self {
        Self::resolve(ProviderOperationCapabilities::default())
    }
}

impl OperationCapabilities {
    pub(crate) fn resolve(provider: ProviderOperationCapabilities) -> Self {
        let provider = if provider.negotiated {
            provider
        } else {
            ProviderOperationCapabilities {
                permission_denial: PermissionDenialCapability::Unknown,
                native_hook_suppression: NativeHookSuppressionCapability::Unknown,
                compaction_reporting: ProviderCompactionReportingCapability::Unknown,
                model_switch_reporting: ProviderModelSwitchReportingCapability::Unknown,
                permission_deferral: ProviderPermissionDeferralCapability::Unknown,
                elicitation_forwarding: ElicitationForwardingCapability::Unknown,
                ..provider
            }
        };
        Self {
            provider,
            compaction_reporting: CompactionReportingCapability::UnsupportedNotImplemented,
            model_switch_reporting: ModelSwitchReportingCapability::UnsupportedNotImplemented,
            permission_deferral: PermissionDeferralCapability::UnsupportedNotImplemented,
            pre_tool_policy: PreToolPolicyCapability::UnsupportedNotImplemented,
            policy_end_turn: PolicyEndTurnCapability::UnsupportedNotImplemented,
            policy_close_session: PolicyCloseSessionCapability::UnsupportedNotImplemented,
            incoming_elicitation: IncomingElicitationCapability::UnsupportedNotImplemented,
        }
    }

    /// Whether provider-derived fields contain a completed negotiation answer.
    pub fn negotiated(self) -> bool {
        self.provider.negotiated
    }

    /// Whether the provider can inject input into an identified active invocation.
    pub fn native_steering(self) -> bool {
        self.provider.native_steering
    }

    /// Whether the provider can restore the same context after connection close.
    pub fn session_resume(self) -> bool {
        self.provider.session_resume
    }

    /// Whether this connection can deliver a user message's images.
    pub fn image_input(self) -> bool {
        self.provider.image_input
    }

    /// Return the provider's scoped permission-denial delivery fact.
    pub fn permission_denial(self) -> PermissionDenialCapability {
        self.provider.permission_denial
    }

    /// Return the effective native executable-hook suppression fact.
    pub fn native_hook_suppression(self) -> NativeHookSuppressionCapability {
        self.provider.native_hook_suppression
    }

    /// Return effective correlated compaction reporting support.
    pub fn compaction_reporting(self) -> CompactionReportingCapability {
        self.compaction_reporting
    }

    /// Return effective validated model-switch reporting support.
    pub fn model_switch_reporting(self) -> ModelSwitchReportingCapability {
        self.model_switch_reporting
    }

    /// Return effective explicit nonterminal permission-deferral support.
    pub fn permission_deferral(self) -> PermissionDeferralCapability {
        self.permission_deferral
    }

    /// Return the provider's MCP elicitation forwarding fact.
    pub fn elicitation_forwarding(self) -> ElicitationForwardingCapability {
        self.provider.elicitation_forwarding
    }

    /// Return effective configured pre-tool policy support.
    pub fn pre_tool_policy(self) -> PreToolPolicyCapability {
        self.pre_tool_policy
    }

    /// Return effective policy end-turn support.
    pub fn policy_end_turn(self) -> PolicyEndTurnCapability {
        self.policy_end_turn
    }

    /// Return effective policy session-close support.
    pub fn policy_close_session(self) -> PolicyCloseSessionCapability {
        self.policy_close_session
    }

    /// Return effective incoming elicitation support.
    pub fn incoming_elicitation(self) -> IncomingElicitationCapability {
        self.incoming_elicitation
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/providers/operation_capabilities.rs"]
mod tests;
