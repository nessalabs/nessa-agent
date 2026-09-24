use super::*;
use crate::agent_install::{
    application::InstallationDelivery,
    domain::{InstallAttempt, PublicationOutcome, PublicationPreparation, PublicationSettlement},
};
use crate::agent_install_test_support::{agent, release, request, temporary_root, PINNED_DIGEST};
use nessa_auth::application::ports::Clock;
use std::{path::Path, sync::Arc};

struct FixedClock;

impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        42
    }
}

fn preparation() -> PublicationPreparation {
    let release = release(
        "1.0.0",
        PINNED_DIGEST,
        &crate::agent_install_test_support::platform(),
    );
    let (mut attempt, _) = InstallAttempt::start(
        agent(),
        crate::agent_install::domain::RuntimeArtifact::for_release(&release),
        request(),
    );
    PublicationPreparation::new(attempt.verified().unwrap()).unwrap()
}

fn delivery_at(root: &Path) -> DurableInstallationDelivery {
    DurableInstallationDelivery::new(root, Path::new("delivery"), Arc::new(FixedClock)).unwrap()
}

#[test]
fn preparation_and_exact_outcome_survive_reopen_until_settled() {
    let root = temporary_root();
    let preparation = preparation();
    let delivery = delivery_at(root.path());
    let prepared = {
        let mut session = delivery.session(request().account_id()).unwrap();
        session.prepare(preparation.clone()).unwrap()
    };
    assert!(matches!(
        delivery
            .session(request().account_id())
            .unwrap()
            .pending()
            .unwrap(),
        Some(PendingInstallationDelivery::Prepared(_))
    ));

    let outcome = PublicationOutcome::no_publication_effect(preparation.clone());
    {
        let mut session = delivery.session(request().account_id()).unwrap();
        session.retain_outcome(&prepared, &outcome).unwrap();
    }
    let mut reopened = delivery.session(request().account_id()).unwrap();
    assert!(matches!(
        reopened.pending().unwrap(),
        Some(PendingInstallationDelivery::Outcome { .. })
    ));
    let settlement = PublicationSettlement::new(&preparation, outcome).unwrap();
    reopened.settle(&prepared, &settlement).unwrap();
    drop(reopened);
    assert!(delivery
        .session(request().account_id())
        .unwrap()
        .pending()
        .unwrap()
        .is_none());
}

#[test]
fn prepared_publication_blocks_a_second_admission_for_the_account() {
    let root = temporary_root();
    let delivery = delivery_at(root.path());
    let mut session = delivery.session(request().account_id()).unwrap();
    session.prepare(preparation()).unwrap();

    let failure = session.prepare(preparation()).unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::Prepare);
}

#[test]
fn a_settlement_without_its_outcome_is_rejected_on_restore() {
    let root = temporary_root();
    let preparation = preparation();
    let delivery = delivery_at(root.path());
    let prepared = {
        let mut session = delivery.session(request().account_id()).unwrap();
        session.prepare(preparation.clone()).unwrap()
    };
    let outcome = PublicationOutcome::no_publication_effect(preparation.clone());
    let settlement = StoredSettlement::new(
        prepared.record_id().to_owned(),
        42,
        &PublicationSettlement::new(&preparation, outcome).unwrap(),
    );
    let encoded = serde_json::to_vec(&settlement).unwrap();
    std::fs::write(
        root.path()
            .join("delivery")
            .join(format!("{}.settled.json", prepared.record_id())),
        encoded,
    )
    .unwrap();

    let failure = delivery
        .session(request().account_id())
        .unwrap()
        .pending()
        .unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::ReadState);
    assert!(failure.detail().contains("no retained outcome"));
}
