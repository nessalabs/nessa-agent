use super::*;
use crate::agent_install::{
    application::{
        InstallDeliveryFailure, InstallDeliveryFailureStage, InstallationDelivery,
        PendingInstallationDelivery,
    },
    domain::{
        InstallAttempt, InstallRequest, InstallTransition, InstallTransitionFacts,
        PublicationOutcome, PublicationPreparation, PublicationSettlement, RollbackState,
        RuntimeArtifact,
    },
};
use crate::agent_install_test_support::{
    agent, platform, release, request, temporary_root, OTHER_DIGEST, PINNED_DIGEST,
};
use nessa_auth::application::ports::Clock;
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Barrier},
};

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

fn terminal_outcome(preparation: &PublicationPreparation) -> PublicationOutcome {
    let verified = preparation.verified();
    let terminal = InstallTransition::restore(
        verified.agent().clone(),
        verified.target().clone(),
        verified.request().clone(),
        InstallTransitionFacts::Installed,
    )
    .unwrap();
    PublicationOutcome::terminal(preparation, terminal).unwrap()
}

fn outcome_with_facts(
    preparation: &PublicationPreparation,
    facts: InstallTransitionFacts,
) -> PublicationOutcome {
    let verified = preparation.verified();
    let terminal = InstallTransition::restore(
        verified.agent().clone(),
        verified.target().clone(),
        verified.request().clone(),
        facts,
    )
    .unwrap();
    PublicationOutcome::terminal(preparation, terminal).unwrap()
}

fn preparation_for(target: RuntimeArtifact, request: InstallRequest) -> PublicationPreparation {
    let (mut attempt, _) = InstallAttempt::start(agent(), target, request);
    PublicationPreparation::new(attempt.verified().unwrap()).unwrap()
}

fn delivery_records(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut records = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.0.cmp(&right.0));
    records
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

#[test]
fn preparation_published_before_acknowledgement_failure_blocks_fresh_session() {
    let root = temporary_root();
    let delivery = delivery_at(root.path());
    let preparation = preparation();
    let failure = delivery
        .open_session(request().account_id())
        .unwrap()
        .prepare_with_acknowledger(preparation.clone(), || {
            Err(InstallDeliveryFailure::new(
                InstallDeliveryFailureStage::Prepare,
                "injected acknowledgement failure".into(),
            ))
        })
        .unwrap_err();

    assert_eq!(failure.stage(), InstallDeliveryFailureStage::Prepare);
    assert_eq!(failure.detail(), "injected acknowledgement failure");
    let mut reopened = delivery_at(root.path())
        .session(request().account_id())
        .unwrap();
    let Some(PendingInstallationDelivery::Prepared(prepared)) = reopened.pending().unwrap() else {
        panic!("the published preparation must remain conservatively unresolved");
    };
    assert_eq!(prepared.preparation(), &preparation);
    assert_eq!(delivery_records(&root.path().join("delivery")).len(), 1);
}

#[test]
fn dropping_session_after_preparation_never_settles_and_blocks_recovery() {
    let root = temporary_root();
    let delivery = delivery_at(root.path());
    let preparation = preparation();
    let prepared = delivery
        .session(request().account_id())
        .unwrap()
        .prepare(preparation.clone())
        .unwrap();

    let mut reopened = delivery_at(root.path())
        .session(request().account_id())
        .unwrap();
    assert_eq!(
        reopened.pending().unwrap(),
        Some(PendingInstallationDelivery::Prepared(prepared))
    );
    assert_eq!(delivery_records(&root.path().join("delivery")).len(), 1);
}

#[test]
fn dropping_session_after_retained_terminal_recovers_the_exact_event() {
    let root = temporary_root();
    let delivery = delivery_at(root.path());
    let preparation = preparation();
    let outcome = terminal_outcome(&preparation);
    let prepared = {
        let mut session = delivery.session(request().account_id()).unwrap();
        let prepared = session.prepare(preparation.clone()).unwrap();
        session.retain_outcome(&prepared, &outcome).unwrap();
        prepared
    };

    let pending = delivery_at(root.path())
        .session(request().account_id())
        .unwrap()
        .pending()
        .unwrap();
    assert_eq!(
        pending,
        Some(PendingInstallationDelivery::Outcome { prepared, outcome })
    );
}

#[test]
fn separate_delivery_instances_serialize_sessions_on_the_named_lock() {
    let root = temporary_root();
    let first = delivery_at(root.path());
    let second = delivery_at(root.path());
    let first_session = first.session(request().account_id()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let (attempting, attempted) = mpsc::channel();
    let (acquired, acquisition) = mpsc::channel();

    std::thread::scope(|threads| {
        let barrier = Arc::clone(&barrier);
        threads.spawn(|| {
            barrier.wait();
            attempting.send(()).unwrap();
            let _session = second.session(request().account_id()).unwrap();
            acquired.send(()).unwrap();
        });
        barrier.wait();
        attempted.recv().unwrap();
        assert!(matches!(
            acquisition.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(first_session);
        acquisition.recv().unwrap();
    });
}

#[test]
fn invalid_live_successors_leave_the_durable_predecessor_unchanged() {
    let root = temporary_root();
    let directory = root.path().join("delivery");
    let delivery = delivery_at(root.path());
    let preparation = preparation();
    let mut session = delivery.session(request().account_id()).unwrap();
    let prepared = session.prepare(preparation.clone()).unwrap();
    let prepared_files = delivery_records(&directory);
    let no_effect = PublicationOutcome::no_publication_effect(preparation.clone());
    let no_effect_settlement = PublicationSettlement::new(&preparation, no_effect).unwrap();

    let failure = session
        .settle(&prepared, &no_effect_settlement)
        .unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::Settle);
    assert!(failure.detail().contains("no retained outcome"));
    assert_eq!(delivery_records(&directory), prepared_files);

    let other_target = RuntimeArtifact::for_release(&release("1.0.1", OTHER_DIGEST, &platform()));
    let conflicting_preparations = [
        preparation_for(
            preparation.verified().target().clone(),
            InstallRequest::new("unix:501", "other-request").unwrap(),
        ),
        preparation_for(other_target, request()),
    ];
    for other_preparation in conflicting_preparations {
        let failure = session
            .retain_outcome(&prepared, &terminal_outcome(&other_preparation))
            .unwrap_err();
        assert_eq!(failure.stage(), InstallDeliveryFailureStage::RetainOutcome);
        assert!(failure.detail().contains("disagrees with its preparation"));
        assert_eq!(delivery_records(&directory), prepared_files);
    }

    let wrong_record = PreparedInstallation::new(
        "00000000-0000-4000-8000-000000000000".into(),
        preparation.clone(),
    );
    let failure = session
        .retain_outcome(&wrong_record, &terminal_outcome(&preparation))
        .unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::RetainOutcome);
    assert!(failure.detail().contains("does not follow"));
    assert_eq!(delivery_records(&directory), prepared_files);

    drop(session);
    let mut wrong_account = delivery.session("unix:502").unwrap();
    let failure = wrong_account
        .retain_outcome(&prepared, &terminal_outcome(&preparation))
        .unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::RetainOutcome);
    assert!(failure.detail().contains("another account"));
    assert_eq!(delivery_records(&directory), prepared_files);
}

#[test]
fn mismatched_terminal_and_no_effect_settlements_preserve_retained_terminal() {
    let root = temporary_root();
    let directory = root.path().join("delivery");
    let delivery = delivery_at(root.path());
    let preparation = preparation();
    let terminal = terminal_outcome(&preparation);
    let mut session = delivery.session(request().account_id()).unwrap();
    let prepared = session.prepare(preparation.clone()).unwrap();
    session.retain_outcome(&prepared, &terminal).unwrap();
    let retained_files = delivery_records(&directory);
    let different_terminal = outcome_with_facts(
        &preparation,
        InstallTransitionFacts::RolledBack(RollbackState::NoInstalledRuntime),
    );
    let settlement = PublicationSettlement::new(&preparation, different_terminal).unwrap();
    let failure = session.settle(&prepared, &settlement).unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::Settle);
    assert!(failure
        .detail()
        .contains("does not follow the retained outcome"));
    assert_eq!(delivery_records(&directory), retained_files);

    let no_effect = PublicationOutcome::no_publication_effect(preparation.clone());
    let settlement = PublicationSettlement::new(&preparation, no_effect).unwrap();

    let failure = session.settle(&prepared, &settlement).unwrap_err();
    assert_eq!(failure.stage(), InstallDeliveryFailureStage::Settle);
    assert!(failure
        .detail()
        .contains("does not follow the retained outcome"));
    assert_eq!(delivery_records(&directory), retained_files);
    drop(session);

    assert_eq!(
        delivery
            .session(request().account_id())
            .unwrap()
            .pending()
            .unwrap(),
        Some(PendingInstallationDelivery::Outcome {
            prepared,
            outcome: terminal
        })
    );
}
