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
        supports_questions: true,
        ..ProviderOperationCapabilities::default()
    }
}

#[test]
fn application_absences_stay_disabled_while_questions_follow_the_verified_binding() {
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
        PolicyEndTurnCapability::Unknown
    );
    assert_eq!(
        capabilities.policy_close_session(),
        PolicyCloseSessionCapability::Unknown
    );
    assert_eq!(
        capabilities.incoming_elicitation(),
        IncomingElicitationCapability::SupportedWithCorrelatedRoundTrip
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
    assert_eq!(
        capabilities.incoming_elicitation(),
        IncomingElicitationCapability::Unknown
    );
    assert_eq!(
        capabilities.policy_end_turn(),
        PolicyEndTurnCapability::Unknown
    );
    assert_eq!(
        capabilities.policy_close_session(),
        PolicyCloseSessionCapability::Unknown
    );
}

#[test]
fn proven_turn_cleanup_is_the_only_fact_that_supports_ending_the_turn() {
    let proven = OperationCapabilities::resolve(ProviderOperationCapabilities {
        negotiated: true,
        turn_stop_cleanup: TurnStopCleanup::ProvenWithSessionReuse,
        session_close_ownership: SessionCloseOwnership::Proven,
        ..ProviderOperationCapabilities::default()
    });
    assert_eq!(
        proven.policy_end_turn(),
        PolicyEndTurnCapability::SupportedForCurrentInvocation
    );
    assert_eq!(
        proven.policy_close_session(),
        PolicyCloseSessionCapability::SupportedForSession
    );
    let unsupported = OperationCapabilities::resolve(ProviderOperationCapabilities {
        negotiated: true,
        turn_stop_cleanup: TurnStopCleanup::Unsupported,
        session_close_ownership: SessionCloseOwnership::Unsupported,
        ..ProviderOperationCapabilities::default()
    });
    assert_eq!(
        unsupported.policy_end_turn(),
        PolicyEndTurnCapability::Unsupported
    );
    assert_eq!(
        unsupported.policy_close_session(),
        PolicyCloseSessionCapability::Unsupported
    );
    assert_eq!(
        OperationCapabilities::default().policy_end_turn(),
        PolicyEndTurnCapability::Unknown
    );
}

#[test]
fn native_hooks_is_a_launch_choice_and_survives_an_unnegotiated_reset() {
    let allowed = OperationCapabilities::resolve(ProviderOperationCapabilities {
        negotiated: false,
        native_hooks_allowed: true,
        ..ProviderOperationCapabilities::default()
    });
    assert!(allowed.native_hooks_allowed());
    assert_eq!(
        allowed.native_hook_suppression(),
        NativeHookSuppressionCapability::Unknown
    );
    assert!(!OperationCapabilities::default().native_hooks_allowed());
}

#[test]
fn negotiated_provider_unsupported_is_preserved() {
    let capabilities = OperationCapabilities::resolve(ProviderOperationCapabilities {
        negotiated: true,
        permission_denial: PermissionDenialCapability::Unsupported,
        native_hook_suppression: NativeHookSuppressionCapability::Unsupported,
        permission_deferral: ProviderPermissionDeferralCapability::Unsupported,
        elicitation_forwarding: ElicitationForwardingCapability::SupportedWithCorrelatedRoundTrip,
        ..ProviderOperationCapabilities::default()
    });
    assert_eq!(
        capabilities.permission_denial(),
        PermissionDenialCapability::Unsupported
    );
    assert_eq!(
        capabilities.native_hook_suppression(),
        NativeHookSuppressionCapability::Unsupported
    );
    assert_eq!(
        capabilities.permission_deferral(),
        PermissionDeferralCapability::Unsupported
    );
    assert_eq!(
        capabilities.incoming_elicitation(),
        IncomingElicitationCapability::Unsupported
    );
    assert_eq!(
        capabilities.elicitation_forwarding(),
        ElicitationForwardingCapability::SupportedWithCorrelatedRoundTrip
    );
}

#[test]
fn offered_effort_levels_count_only_once_negotiated() {
    use crate::domain::model_metadata::value_objects::{EffortLevel, EffortLevels};
    let levels = EffortLevels::new(
        ["low", "high"]
            .into_iter()
            .map(|name| EffortLevel::new(name.into()).unwrap())
            .collect(),
    )
    .unwrap();
    let offered = levels.offered_by(["low", "high"]);
    let reported = |negotiated| ProviderOperationCapabilities {
        effort_levels: offered,
        ..every_provider_feature_supported(negotiated)
    };
    assert_eq!(
        OperationCapabilities::resolve(reported(true)).effort_levels(),
        offered
    );
    // Opening or restoring: whatever an earlier connection offered is not claimed.
    assert!(OperationCapabilities::resolve(reported(false))
        .effort_levels()
        .is_empty());
    assert!(OperationCapabilities::default().effort_levels().is_empty());
}
