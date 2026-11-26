use super::observe_directory_policy;
use crate::{
    application::{DirectoryOperation, DirectoryPolicyError},
    domain::{
        DirectoryCapabilities, DirectoryCapability, DirectoryCommandRule, DirectoryGrant,
        DirectoryGrantState, DirectoryResource, DirectoryRuleEffect, DirectorySubject,
        MAX_DIRECTORY_ARGV_ITEMS, MAX_DIRECTORY_ARGV_PATTERN_BYTES, MAX_DIRECTORY_PATH_BYTES,
        MAX_DIRECTORY_RULES,
    },
};

fn live(values: &[DirectoryCapability], rules: Vec<DirectoryCommandRule>) -> DirectoryGrantState {
    DirectoryGrantState::Live {
        grant: DirectoryGrant::new(DirectoryCapabilities::new(values).unwrap(), rules).unwrap(),
        revision: 1,
    }
}

fn rule(effect: DirectoryRuleEffect, argv: &[&str]) -> DirectoryCommandRule {
    DirectoryCommandRule {
        effect,
        program: "fixture".into(),
        argv: argv.iter().map(|value| (*value).into()).collect(),
    }
}

fn exec(within: bool, enforced_boundary: bool, argv: &[&str]) -> DirectoryOperation {
    DirectoryOperation::Exec {
        within,
        enforced_boundary,
        program: "fixture".into(),
        argv: argv.iter().map(|value| (*value).into()).collect(),
    }
}

#[test]
fn resource_identity_and_grant_inputs_are_bounded() {
    assert_ne!(
        DirectorySubject::Principal("same".into()),
        DirectorySubject::Receiver("same".into())
    );
    let path = format!("/{}", "a".repeat(MAX_DIRECTORY_PATH_BYTES - 1));
    assert!(DirectoryResource::from_canonical("local".into(), path.clone()).is_ok());
    assert!(DirectoryResource::from_canonical("local".into(), format!("{path}a")).is_err());
    assert!(DirectoryCapabilities::new(&[]).is_err());
    let caps = DirectoryCapabilities::new(&[DirectoryCapability::Exec]).unwrap();
    let maximum = rule(DirectoryRuleEffect::Allow, &[]);
    assert!(DirectoryGrant::new(caps, vec![maximum.clone(); MAX_DIRECTORY_RULES]).is_ok());
    assert!(DirectoryGrant::new(caps, vec![maximum; MAX_DIRECTORY_RULES + 1]).is_err());
    let pattern = "a".repeat(MAX_DIRECTORY_ARGV_PATTERN_BYTES + 1);
    assert!(DirectoryGrant::new(
        caps,
        vec![rule(DirectoryRuleEffect::Allow, &[pattern.as_str()])]
    )
    .is_err());
}

#[test]
fn cedar_requires_live_grant_inside_the_boundary_and_exact_path_capability() {
    let within = DirectoryOperation::Read { within: true };
    for state in [
        DirectoryGrantState::Absent,
        DirectoryGrantState::Revoked { revision: 1 },
    ] {
        assert!(!observe_directory_policy(state, &within).unwrap().permitted);
    }
    let read = live(&[DirectoryCapability::Read], vec![]);
    assert!(
        observe_directory_policy(read.clone(), &within)
            .unwrap()
            .permitted
    );
    assert!(
        !observe_directory_policy(read.clone(), &DirectoryOperation::Read { within: false })
            .unwrap()
            .permitted
    );
    assert!(
        !observe_directory_policy(read, &DirectoryOperation::Write { within: true })
            .unwrap()
            .permitted
    );
    let write = live(&[DirectoryCapability::Write], vec![]);
    assert!(
        observe_directory_policy(write, &DirectoryOperation::Write { within: true })
            .unwrap()
            .permitted
    );
}

#[test]
fn cedar_forbid_beats_permit_and_exec_needs_an_enforced_boundary() {
    let allow = rule(DirectoryRuleEffect::Allow, &[]);
    let deny = rule(DirectoryRuleEffect::Deny, &["blocked*"]);
    for rules in [vec![allow.clone(), deny.clone()], vec![deny, allow]] {
        let state = live(&[DirectoryCapability::Exec], rules);
        let denied =
            observe_directory_policy(state.clone(), &exec(true, true, &["blocked-value"])).unwrap();
        assert!(!denied.permitted);
        assert!(denied.reasons.iter().any(|id| id.ends_with(".forbid")));
        assert!(
            observe_directory_policy(state.clone(), &exec(true, true, &["okay", "second"]))
                .unwrap()
                .permitted
        );
        assert!(
            !observe_directory_policy(state, &exec(true, false, &["okay"]))
                .unwrap()
                .permitted
        );
    }
    let too_many = vec!["ordinary"; MAX_DIRECTORY_ARGV_ITEMS + 1];
    assert_eq!(
        observe_directory_policy(DirectoryGrantState::Absent, &exec(true, true, &too_many)),
        Err(DirectoryPolicyError::InvalidFacts)
    );
}
