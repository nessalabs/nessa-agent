use super::*;
use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, ConfiguredRefusal,
};
use crate::application::agent_execution::providers::{
    OperationCapabilities, PolicyCloseSessionCapability, PolicyEndTurnCapability,
    ProviderOperationCapabilities, SessionCloseOwnership, TurnStopCleanup,
};
use crate::domain::agent_execution::permissions::RefusalOutcome;

fn rule() -> ApprovalRuleReference {
    ApprovalRuleReference::new(
        "refuse-shell",
        "1",
        ActionContext::new("host", "policy", "grant").unwrap(),
    )
    .unwrap()
}

fn capabilities(turn: TurnStopCleanup, session: SessionCloseOwnership) -> OperationCapabilities {
    OperationCapabilities::resolve(ProviderOperationCapabilities {
        negotiated: true,
        turn_stop_cleanup: turn,
        session_close_ownership: session,
        ..ProviderOperationCapabilities::default()
    })
}

#[test]
fn continue_is_admitted_without_a_rule_on_every_binding() {
    let configured = ConfiguredRefusal::continue_turn();
    assert_eq!(configured.outcome(), RefusalOutcome::Continue);
    assert!(configured.rule().is_none());
    assert!(admit_configured_outcome(&configured, OperationCapabilities::default()).is_ok());
    assert!(admit_configured_outcome(
        &configured,
        capabilities(TurnStopCleanup::Unknown, SessionCloseOwnership::Unknown),
    )
    .is_ok());
}

#[test]
fn a_stopping_outcome_names_its_rule_and_cannot_mean_continue() {
    let ended = ConfiguredRefusal::stopping(RefusalOutcome::EndTurn, rule()).unwrap();
    assert_eq!(ended.outcome(), RefusalOutcome::EndTurn);
    assert_eq!(ended.rule().unwrap().rule_id(), "refuse-shell");
    assert!(matches!(
        ConfiguredRefusal::stopping(RefusalOutcome::Continue, rule()),
        Err(AgentError::InvalidInput(_))
    ));
}

#[test]
fn ending_the_turn_is_admitted_only_when_cleanup_and_reuse_are_proven() {
    let configured = ConfiguredRefusal::stopping(RefusalOutcome::EndTurn, rule()).unwrap();
    assert!(admit_configured_outcome(
        &configured,
        capabilities(
            TurnStopCleanup::ProvenWithSessionReuse,
            SessionCloseOwnership::Unknown,
        ),
    )
    .is_ok());
    for (turn, session) in [
        (TurnStopCleanup::Unknown, SessionCloseOwnership::Proven),
        (TurnStopCleanup::Unsupported, SessionCloseOwnership::Proven),
    ] {
        let error = admit_configured_outcome(&configured, capabilities(turn, session))
            .expect_err("unproven end-turn must be refused");
        assert!(
            matches!(error, AgentError::Configuration(ref text) if text.contains("ends the turn")),
            "{error:?} did not refuse the end-turn outcome"
        );
        assert!(
            capabilities(turn, session).policy_close_session()
                == PolicyCloseSessionCapability::SupportedForSession
                || capabilities(turn, session).policy_end_turn()
                    != PolicyEndTurnCapability::SupportedForCurrentInvocation
        );
        assert_ne!(
            capabilities(turn, session).policy_end_turn(),
            PolicyEndTurnCapability::SupportedForCurrentInvocation
        );
    }
}

#[test]
fn closing_the_session_is_not_how_an_unproven_end_turn_is_honored() {
    let end_turn = ConfiguredRefusal::stopping(RefusalOutcome::EndTurn, rule()).unwrap();
    let close = ConfiguredRefusal::stopping(RefusalOutcome::CloseSession, rule()).unwrap();
    let can_close = capabilities(TurnStopCleanup::Unknown, SessionCloseOwnership::Proven);
    assert!(admit_configured_outcome(&end_turn, can_close).is_err());
    assert!(admit_configured_outcome(&close, can_close).is_ok());
    assert_eq!(end_turn.outcome(), RefusalOutcome::EndTurn);
}

#[test]
fn closing_the_session_requires_proven_ownership() {
    let configured = ConfiguredRefusal::stopping(RefusalOutcome::CloseSession, rule()).unwrap();
    assert!(admit_configured_outcome(
        &configured,
        capabilities(
            TurnStopCleanup::ProvenWithSessionReuse,
            SessionCloseOwnership::Unknown
        ),
    )
    .is_err());
    assert!(admit_configured_outcome(
        &configured,
        capabilities(TurnStopCleanup::Unknown, SessionCloseOwnership::Proven),
    )
    .is_ok());
}
