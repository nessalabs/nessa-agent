use super::*;
use crate::agent_install::domain::{InstallAttempt, InstallTransitionFacts, RollbackState};
use crate::agent_install_test_support::{agent, platform, release, request, PINNED_DIGEST};

fn attempt() -> InstallAttempt {
    let release = release("1.0.0", PINNED_DIGEST, &platform());
    InstallAttempt::start(
        agent(),
        crate::agent_install::domain::RuntimeArtifact::for_release(&release),
        request(),
    )
    .0
}

#[test]
fn only_verified_evidence_can_prepare_publication() {
    let release = release("1.0.0", PINNED_DIGEST, &platform());
    let (_, started) = InstallAttempt::start(
        agent(),
        crate::agent_install::domain::RuntimeArtifact::for_release(&release),
        request(),
    );
    assert_eq!(
        PublicationPreparation::new(started),
        Err(PublicationDeliveryError::PreparationIsNotVerified)
    );
}

#[test]
fn terminal_must_belong_to_the_exact_prepared_attempt() {
    let mut first = attempt();
    let preparation = PublicationPreparation::new(first.verified().unwrap()).unwrap();
    let mut other = InstallAttempt::start(
        agent(),
        preparation.verified().target().clone(),
        crate::agent_install::domain::InstallRequest::new("unix:501", "other").unwrap(),
    )
    .0;
    other.verified().unwrap();
    let terminal = other.installed().unwrap();

    assert!(matches!(
        PublicationOutcome::terminal(&preparation, terminal),
        Err(PublicationDeliveryError::InvalidAttempt(_))
    ));
}

#[test]
fn settlement_requires_the_exact_retained_predecessor() {
    let mut first = attempt();
    let preparation = PublicationPreparation::new(first.verified().unwrap()).unwrap();
    let terminal = first.installed().unwrap();
    let outcome = PublicationOutcome::terminal(&preparation, terminal).unwrap();

    let mut other = InstallAttempt::start(
        agent(),
        preparation.verified().target().clone(),
        crate::agent_install::domain::InstallRequest::new("unix:501", "other").unwrap(),
    )
    .0;
    let other_preparation = PublicationPreparation::new(other.verified().unwrap()).unwrap();
    assert_eq!(
        PublicationSettlement::new(&other_preparation, outcome),
        Err(PublicationDeliveryError::ConflictingPreparation)
    );
}

#[test]
fn no_effect_is_distinct_from_every_terminal_fact() {
    let mut attempt = attempt();
    let preparation = PublicationPreparation::new(attempt.verified().unwrap()).unwrap();
    let no_effect = PublicationOutcome::no_publication_effect(preparation.clone());
    assert!(no_effect.terminal_transition().is_none());

    let rolled_back = attempt
        .rolled_back(RollbackState::NoInstalledRuntime)
        .unwrap();
    assert!(matches!(
        rolled_back.facts(),
        InstallTransitionFacts::RolledBack { .. }
    ));
    assert_ne!(
        no_effect,
        PublicationOutcome::terminal(&preparation, rolled_back).unwrap()
    );
}
