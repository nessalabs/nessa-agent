use super::*;

fn every_provider_feature_supported(negotiated: bool) -> ProviderOperationCapabilities {
    ProviderOperationCapabilities {
        negotiated,
        permission_denial: PermissionDenialCapability::SupportedForOfferedPermissionReviews,
        native_hook_suppression: NativeHookSuppressionCapability::SupportedForUserConfiguredHooks,
        compaction_reporting:
            ProviderCompactionReportingCapability::SupportedWithInvocationCorrelation,
        model_switch_reporting:
            ProviderModelSwitchReportingCapability::SupportedAfterValidatedSwitch,
        permission_deferral: ProviderPermissionDeferralCapability::SupportedWithNonterminalOutcome,
        elicitation_forwarding: ElicitationForwardingCapability::SupportedWithCorrelatedRoundTrip,
        ..ProviderOperationCapabilities::default()
    }
}

#[test]
fn application_absences_cannot_be_enabled_by_provider_advertisements() {
    let capabilities = OperationCapabilities::resolve(every_provider_feature_supported(true));
    assert_eq!(
        capabilities.permission_denial(),
        PermissionDenialCapability::SupportedForOfferedPermissionReviews
    );
    assert_eq!(
        capabilities.compaction_reporting(),
        CompactionReportingCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.model_switch_reporting(),
        ModelSwitchReportingCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.permission_deferral(),
        PermissionDeferralCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.pre_tool_policy(),
        PreToolPolicyCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.policy_end_turn(),
        PolicyEndTurnCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.policy_close_session(),
        PolicyCloseSessionCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.incoming_elicitation(),
        IncomingElicitationCapability::UnsupportedNotImplemented
    );
}

#[test]
fn provider_features_are_unknown_until_negotiation_completes() {
    let capabilities = OperationCapabilities::resolve(every_provider_feature_supported(false));
    assert_eq!(
        capabilities.permission_denial(),
        PermissionDenialCapability::Unknown
    );
    assert_eq!(
        capabilities.native_hook_suppression(),
        NativeHookSuppressionCapability::Unknown
    );
    assert_eq!(
        capabilities.compaction_reporting(),
        CompactionReportingCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.model_switch_reporting(),
        ModelSwitchReportingCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.permission_deferral(),
        PermissionDeferralCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.elicitation_forwarding(),
        ElicitationForwardingCapability::Unknown
    );
}

#[test]
fn negotiated_provider_unsupported_is_preserved() {
    let capabilities = OperationCapabilities::resolve(ProviderOperationCapabilities {
        negotiated: true,
        permission_denial: PermissionDenialCapability::Unsupported,
        permission_deferral: ProviderPermissionDeferralCapability::Unsupported,
        ..ProviderOperationCapabilities::default()
    });
    assert_eq!(
        capabilities.permission_denial(),
        PermissionDenialCapability::Unsupported
    );
    assert_eq!(
        capabilities.permission_deferral(),
        PermissionDeferralCapability::UnsupportedNotImplemented
    );
}
