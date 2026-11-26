use super::CedarDirectoryPolicyEvaluator;
use crate::{
    application::{
        DirectoryCommand, DirectoryDenialReason, DirectoryOperation, DirectoryPolicyAnswer,
        DirectoryPolicyError, DirectoryPolicyEvaluator,
    },
    domain::{
        DirectoryCapabilities, DirectoryCapability, DirectoryCommandRule, DirectoryGrant,
        DirectoryGrantState, DirectoryResource, DirectoryRuleEffect, DirectorySubject,
        DirectorySubjectKind, MAX_DIRECTORY_ARGV_ITEMS, MAX_DIRECTORY_ARGV_PATTERN_BYTES,
        MAX_DIRECTORY_PATH_BYTES, MAX_DIRECTORY_RULES, MAX_DIRECTORY_SELECTOR_BYTES,
    },
};

fn live(values: &[DirectoryCapability], rules: Vec<DirectoryCommandRule>) -> DirectoryGrantState {
    DirectoryGrantState::Live {
        grant: DirectoryGrant::new(DirectoryCapabilities::new(values).unwrap(), rules).unwrap(),
        revision: 1,
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn rule(effect: DirectoryRuleEffect, argv: &[&str]) -> DirectoryCommandRule {
    DirectoryCommandRule::new(effect, "fixture".into(), args(argv)).unwrap()
}

fn exec(within: bool, enforced_boundary: bool, argv: &[&str]) -> DirectoryOperation {
    DirectoryOperation::Exec {
        within,
        enforced_boundary,
        command: DirectoryCommand::new("fixture".into(), args(argv)).unwrap(),
    }
}

fn observe(state: &DirectoryGrantState, operation: &DirectoryOperation) -> DirectoryPolicyAnswer {
    let evaluator: &dyn DirectoryPolicyEvaluator = &CedarDirectoryPolicyEvaluator;
    evaluator.evaluate(state, operation).unwrap()
}

fn default_denial() -> DirectoryPolicyAnswer {
    DirectoryPolicyAnswer::Deny(DirectoryDenialReason::DefaultDeny)
}

#[test]
fn resource_identity_and_grant_inputs_are_bounded() {
    let principal = DirectorySubject::new(DirectorySubjectKind::Principal, "same".into()).unwrap();
    let receiver = DirectorySubject::new(DirectorySubjectKind::Receiver, "same".into()).unwrap();
    assert_ne!(principal, receiver);
    assert_eq!(principal.kind(), DirectorySubjectKind::Principal);
    assert_eq!(principal.id(), receiver.id());
    let bounded = "é".repeat(MAX_DIRECTORY_SELECTOR_BYTES / 2);
    assert!(DirectorySubject::new(DirectorySubjectKind::Receiver, bounded.clone()).is_ok());
    for invalid in [" ".into(), "bad\nrecord".into(), format!("{bounded}x")] {
        assert!(DirectorySubject::new(DirectorySubjectKind::Receiver, invalid).is_err());
    }

    let path = format!("/{}", "a".repeat(MAX_DIRECTORY_PATH_BYTES - 1));
    assert!(DirectoryResource::from_canonical("local".into(), path.clone()).is_ok());
    assert!(DirectoryResource::from_canonical("local".into(), format!("{path}a")).is_err());
    assert!(DirectoryCapabilities::new(&[]).is_err());
    let caps = DirectoryCapabilities::new(&[DirectoryCapability::Exec]).unwrap();
    let maximum = rule(DirectoryRuleEffect::Allow, &[]);
    assert!(DirectoryGrant::new(caps, vec![maximum.clone(); MAX_DIRECTORY_RULES]).is_ok());
    assert!(DirectoryGrant::new(caps, vec![maximum; MAX_DIRECTORY_RULES + 1]).is_err());
    let pattern = "é".repeat(MAX_DIRECTORY_ARGV_PATTERN_BYTES / 2);
    assert!(DirectoryCommandRule::new(
        DirectoryRuleEffect::Allow,
        "fixture".into(),
        vec![pattern.clone()]
    )
    .is_ok());
    assert!(DirectoryCommandRule::new(
        DirectoryRuleEffect::Allow,
        "fixture".into(),
        vec![format!("{pattern}x")]
    )
    .is_err());
    assert_eq!(
        DirectoryCommand::new("fixture".into(), vec!["arg".into(); MAX_DIRECTORY_ARGV_ITEMS + 1]),
        Err(DirectoryPolicyError::InvalidFacts)
    );
}

#[test]
fn cedar_requires_live_grant_inside_the_boundary_and_exact_path_capability() {
    let within = DirectoryOperation::Read { within: true };
    for state in [
        DirectoryGrantState::Absent,
        DirectoryGrantState::Revoked { revision: 1 },
    ] {
        assert_eq!(observe(&state, &within), default_denial());
    }
    let read = live(&[DirectoryCapability::Read], vec![]);
    assert!(matches!(
        observe(&read, &within),
        DirectoryPolicyAnswer::Permit { policy_ids } if !policy_ids.is_empty()
    ));
    assert_eq!(
        observe(&read, &DirectoryOperation::Read { within: false }),
        default_denial()
    );
    assert_eq!(
        observe(&read, &DirectoryOperation::Write { within: true }),
        default_denial()
    );
    let write = live(&[DirectoryCapability::Write], vec![]);
    assert!(matches!(
        observe(&write, &DirectoryOperation::Write { within: true }),
        DirectoryPolicyAnswer::Permit { .. }
    ));
    assert_eq!(
        CedarDirectoryPolicyEvaluator.evaluate(
            &DirectoryGrantState::Revoked { revision: 0 },
            &within
        ),
        Err(DirectoryPolicyError::InvalidFacts)
    );
}

#[test]
fn cedar_matched_forbid_is_distinct_from_default_deny_in_both_rule_orders() {
    let allow = rule(DirectoryRuleEffect::Allow, &[]);
    let deny = rule(DirectoryRuleEffect::Deny, &["blocked*"]);
    for rules in [vec![allow.clone(), deny.clone()], vec![deny, allow]] {
        let state = live(&[DirectoryCapability::Exec], rules);
        assert!(matches!(
            observe(&state, &exec(true, true, &["blocked-value"])),
            DirectoryPolicyAnswer::Deny(DirectoryDenialReason::MatchedForbids { policy_ids })
                if policy_ids.len() == 1 && policy_ids[0].ends_with(".forbid")
        ));
        assert!(matches!(
            observe(&state, &exec(true, true, &["ordinary", "second"])),
            DirectoryPolicyAnswer::Permit { .. }
        ));
        assert_eq!(
            observe(&state, &exec(true, false, &["ordinary"])),
            default_denial()
        );
    }
}
