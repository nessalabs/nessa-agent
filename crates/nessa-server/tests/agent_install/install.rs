use super::*;
use crate::agent_install::application::{
    AuditAcknowledgement, AuditFailureStage, InstallDeliveryFailure, InstallDeliveryFailureStage,
    InstallationDelivery, InstallationDeliverySession, PendingInstallationDelivery,
    PreparedInstallation, Publication, PublicationCleanupFailure, PublishFailure, RollbackChange,
};
use crate::agent_install::domain::{
    ArchiveDigest, InstallEventSlot, InstallFailureEvidence, InstallFailureKind, InstallRequest,
    InstallTransitionError, InstallTransitionFacts, InstallTransitionKind, Libc,
    PublicationOutcome, PublicationPreparation, PublicationSettlement, RecoveryState,
    ReleasePlatform, ReleaseRequirements, RollbackState, RuntimeArtifact,
};
use crate::agent_install::infrastructure::{DurableInstallAudit, DurableInstallationDelivery};
use crate::agent_install_test_support::{
    agent, audit, delivery, host, host_of, platform, release, release_needing, request,
    temporary_root, FakeSource, FakeStore, RecordingAudit, OTHER_DIGEST, PINNED_DIGEST,
};
use nessa_auth::application::ports::Clock;
use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

struct BlockingAudit {
    target: InstallTransitionKind,
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    fail: bool,
}

struct FailOnceAudit(Mutex<bool>);

struct FixedClock;

impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        42
    }
}

struct CommitTerminalThenFailOnceAudit {
    durable: DurableInstallAudit,
    failed: Mutex<bool>,
}

struct RefuseTerminalBeforeCommitOnceAudit {
    durable: DurableInstallAudit,
    failed: Mutex<bool>,
}

impl InstallAudit for RefuseTerminalBeforeCommitOnceAudit {
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        let mut failed = self.failed.lock().unwrap();
        if transition.kind() == InstallTransitionKind::Installed && !*failed {
            *failed = true;
            return Err(AuditFailure::new(
                AuditFailureStage::PublishRecord,
                "injected failure before durable commit".into(),
                None,
                None,
            ));
        }
        drop(failed);
        self.durable.record(transition)
    }

    fn completion_for(
        &self,
        preparation: &PublicationPreparation,
    ) -> Result<Option<InstallTransition>, AuditFailure> {
        self.durable.completion_for(preparation)
    }
}

struct SignallingLease(mpsc::Sender<()>);

impl Drop for SignallingLease {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

struct LeaseCheckingAudit {
    target: InstallTransitionKind,
    dropped: Mutex<mpsc::Receiver<()>>,
    recorded: Mutex<Vec<InstallTransition>>,
    fail: bool,
}

impl InstallAudit for LeaseCheckingAudit {
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        if transition.kind() == self.target {
            assert!(matches!(
                self.dropped.lock().unwrap().try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
            self.recorded.lock().unwrap().push(transition.clone());
            if self.fail {
                return Err(AuditFailure::new(
                    AuditFailureStage::AcknowledgeRecord,
                    "sink refused".into(),
                    None,
                    None,
                ));
            }
        }
        Ok(AuditAcknowledgement::Recorded)
    }
}

struct OneShotFailureStore {
    inner: FakeStore,
    failure: Mutex<Option<PublishFailure>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DeliveryCall {
    Pending,
    Prepare(PublicationPreparation),
    Retain(PublicationOutcome),
    Settle(PublicationSettlement),
}

#[derive(Default)]
struct ScriptedDeliveryState {
    pending: Option<PendingInstallationDelivery>,
    calls: Vec<DeliveryCall>,
    prepare_failures: usize,
    retain_failures: usize,
    settle_failures: usize,
}

struct ScriptedDelivery {
    state: Mutex<ScriptedDeliveryState>,
    lease_dropped: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
    lease_checks_remaining: Mutex<usize>,
}

impl ScriptedDelivery {
    fn new(
        prepare_failures: usize,
        retain_failures: usize,
        settle_failures: usize,
        lease_dropped: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
    ) -> Self {
        Self {
            state: Mutex::new(ScriptedDeliveryState {
                pending: None,
                calls: Vec::new(),
                prepare_failures,
                retain_failures,
                settle_failures,
            }),
            lease_dropped,
            lease_checks_remaining: Mutex::new(1),
        }
    }

    fn calls(&self) -> Vec<DeliveryCall> {
        self.state.lock().unwrap().calls.clone()
    }

    fn pending(&self) -> Option<PendingInstallationDelivery> {
        self.state.lock().unwrap().pending.clone()
    }

    fn assert_publication_lease_held(&self) {
        let mut remaining = self.lease_checks_remaining.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
        } else {
            return;
        }
        if let Some(dropped) = &self.lease_dropped {
            assert!(matches!(
                dropped.lock().unwrap().try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
    }
}

struct ScriptedDeliverySession<'a> {
    delivery: &'a ScriptedDelivery,
}

impl InstallationDelivery for ScriptedDelivery {
    fn session(
        &self,
        _account_id: &str,
    ) -> Result<Box<dyn InstallationDeliverySession + '_>, InstallDeliveryFailure> {
        Ok(Box::new(ScriptedDeliverySession { delivery: self }))
    }
}

impl InstallationDeliverySession for ScriptedDeliverySession<'_> {
    fn pending(&mut self) -> Result<Option<PendingInstallationDelivery>, InstallDeliveryFailure> {
        let mut state = self.delivery.state.lock().unwrap();
        state.calls.push(DeliveryCall::Pending);
        Ok(state.pending.clone())
    }

    fn prepare(
        &mut self,
        preparation: PublicationPreparation,
    ) -> Result<PreparedInstallation, InstallDeliveryFailure> {
        let mut state = self.delivery.state.lock().unwrap();
        state.calls.push(DeliveryCall::Prepare(preparation.clone()));
        if state.prepare_failures > 0 {
            state.prepare_failures -= 1;
            return Err(InstallDeliveryFailure::new(
                InstallDeliveryFailureStage::Prepare,
                "injected preparation failure".into(),
            ));
        }
        if state.pending.is_some() {
            return Err(InstallDeliveryFailure::new(
                InstallDeliveryFailureStage::Prepare,
                "another publication is unresolved".into(),
            ));
        }
        let prepared = PreparedInstallation::new(
            format!("prepared-{}", preparation.verified().request().request_id()),
            preparation,
        );
        state.pending = Some(PendingInstallationDelivery::Prepared(prepared.clone()));
        Ok(prepared)
    }

    fn retain_outcome(
        &mut self,
        prepared: &PreparedInstallation,
        outcome: &PublicationOutcome,
    ) -> Result<(), InstallDeliveryFailure> {
        self.delivery.assert_publication_lease_held();
        let mut state = self.delivery.state.lock().unwrap();
        state.calls.push(DeliveryCall::Retain(outcome.clone()));
        if state.retain_failures > 0 {
            state.retain_failures -= 1;
            return Err(InstallDeliveryFailure::new(
                InstallDeliveryFailureStage::RetainOutcome,
                "injected outcome retention failure".into(),
            ));
        }
        state.pending = Some(PendingInstallationDelivery::Outcome {
            prepared: prepared.clone(),
            outcome: outcome.clone(),
        });
        Ok(())
    }

    fn settle(
        &mut self,
        _prepared: &PreparedInstallation,
        settlement: &PublicationSettlement,
    ) -> Result<(), InstallDeliveryFailure> {
        let mut state = self.delivery.state.lock().unwrap();
        state.calls.push(DeliveryCall::Settle(settlement.clone()));
        if state.settle_failures > 0 {
            state.settle_failures -= 1;
            return Err(InstallDeliveryFailure::new(
                InstallDeliveryFailureStage::Settle,
                "injected settlement failure".into(),
            ));
        }
        state.pending = None;
        Ok(())
    }
}

struct ScriptedAudit {
    records: Mutex<Vec<InstallTransition>>,
    fail_terminals: bool,
    replay_started_after_terminal_replay: bool,
    lease_dropped: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
    lease_checks_remaining: Mutex<usize>,
}

impl ScriptedAudit {
    fn new(
        fail_terminals: bool,
        replay_started_after_terminal_replay: bool,
        lease_dropped: Option<Arc<Mutex<mpsc::Receiver<()>>>>,
    ) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            fail_terminals,
            replay_started_after_terminal_replay,
            lease_dropped,
            lease_checks_remaining: Mutex::new(1),
        }
    }

    fn records(&self) -> Vec<InstallTransition> {
        self.records.lock().unwrap().clone()
    }

    fn terminal_records(&self) -> Vec<InstallTransition> {
        self.records()
            .into_iter()
            .filter(|record| record.event_identity().slot() == InstallEventSlot::CompletionOutcome)
            .collect()
    }

    fn assert_publication_lease_held(&self) {
        let mut remaining = self.lease_checks_remaining.lock().unwrap();
        if *remaining > 0 {
            *remaining -= 1;
        } else {
            return;
        }
        if let Some(dropped) = &self.lease_dropped {
            assert!(matches!(
                dropped.lock().unwrap().try_recv(),
                Err(mpsc::TryRecvError::Empty)
            ));
        }
    }
}

impl InstallAudit for ScriptedAudit {
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        let terminal = transition.event_identity().slot() == InstallEventSlot::CompletionOutcome;
        if terminal {
            self.assert_publication_lease_held();
        }
        let mut records = self.records.lock().unwrap();
        let terminal_count = records
            .iter()
            .filter(|record| record.event_identity().slot() == InstallEventSlot::CompletionOutcome)
            .count();
        if transition.kind() == InstallTransitionKind::Started
            && self.replay_started_after_terminal_replay
            && terminal_count >= 2
        {
            records.push(transition);
            return Ok(AuditAcknowledgement::Replayed);
        }
        records.push(transition);
        if terminal && self.fail_terminals {
            return Err(AuditFailure::new(
                AuditFailureStage::AcknowledgeRecord,
                "injected terminal audit failure".into(),
                None,
                None,
            ));
        }
        Ok(AuditAcknowledgement::Recorded)
    }

    fn completion_for(
        &self,
        preparation: &PublicationPreparation,
    ) -> Result<Option<InstallTransition>, AuditFailure> {
        Ok(self.records.lock().unwrap().iter().find_map(|record| {
            (record.event_identity().slot() == InstallEventSlot::CompletionOutcome
                && record.agent() == preparation.verified().agent()
                && record.target() == preparation.verified().target()
                && record.request() == preparation.verified().request())
            .then(|| record.clone())
        }))
    }
}

#[derive(Clone, Copy, Debug)]
enum TerminalPublicationCase {
    Installed,
    Replaced,
    RolledBack,
    RecoveryIncomplete,
}

impl TerminalPublicationCase {
    fn expected_kind(self) -> InstallTransitionKind {
        match self {
            Self::Installed => InstallTransitionKind::Installed,
            Self::Replaced => InstallTransitionKind::Replaced,
            Self::RolledBack => InstallTransitionKind::RolledBack,
            Self::RecoveryIncomplete => InstallTransitionKind::RecoveryIncomplete,
        }
    }

    fn expected_runtime_state(self) -> RuntimeStateEvidence {
        match self {
            Self::Installed | Self::Replaced => RuntimeStateEvidence::TargetInstalled,
            Self::RolledBack => RuntimeStateEvidence::NoInstalledRuntime,
            Self::RecoveryIncomplete => RuntimeStateEvidence::Unconfirmed,
        }
    }
}

fn assert_terminal_case(
    case: TerminalPublicationCase,
    terminal: &InstallTransition,
    request: &InstallRequest,
    target: &RuntimeArtifact,
    previous: &RuntimeArtifact,
) {
    assert_eq!(terminal.kind(), case.expected_kind());
    assert_eq!(terminal.request(), request);
    assert_eq!(terminal.target(), target);
    match case {
        TerminalPublicationCase::Installed => {
            assert_eq!(terminal.facts(), &InstallTransitionFacts::Installed);
        }
        TerminalPublicationCase::Replaced => {
            assert_eq!(terminal.previous(), Some(previous));
        }
        TerminalPublicationCase::RolledBack => {
            assert_eq!(
                terminal.rollback(),
                Some(&RollbackState::NoInstalledRuntime)
            );
        }
        TerminalPublicationCase::RecoveryIncomplete => {
            let (state, failures) = terminal.recovery().unwrap();
            assert_eq!(state, &RecoveryState::Unconfirmed);
            assert_eq!(
                failures.publication().kind(),
                InstallFailureKind::Unwritable
            );
            assert_eq!(failures.withdrawal(), None);
            assert_eq!(failures.restoration(), None);
            assert_eq!(
                failures.confirmation().unwrap().kind(),
                InstallFailureKind::MalformedArchive
            );
        }
    }
}

fn terminal_store(
    case: TerminalPublicationCase,
    root: &Path,
    previous: &RuntimeArtifact,
    lease_dropped: mpsc::Sender<()>,
) -> Box<dyn RuntimeStore> {
    match case {
        TerminalPublicationCase::Installed => {
            Box::new(FakeStore::empty(root).signalling_lease_drop(lease_dropped))
        }
        TerminalPublicationCase::Replaced => Box::new(
            FakeStore::empty(root)
                .replacing(previous.clone())
                .signalling_lease_drop(lease_dropped),
        ),
        TerminalPublicationCase::RolledBack => Box::new(OneShotFailureStore {
            inner: FakeStore::empty(root),
            failure: Mutex::new(Some(PublishFailure::rolled_back(
                StoreFailure::Unwritable("publication failed".into()),
                RollbackChange::NoInstalledRuntime,
                Box::new(SignallingLease(lease_dropped)),
            ))),
        }),
        TerminalPublicationCase::RecoveryIncomplete => {
            let cleanup = PublicationCleanupFailure::new(
                None,
                None,
                Some(StoreFailure::MalformedArchive(
                    "remaining runtime could not be confirmed".into(),
                )),
            )
            .unwrap();
            Box::new(OneShotFailureStore {
                inner: FakeStore::empty(root),
                failure: Mutex::new(Some(PublishFailure::incomplete(
                    StoreFailure::Unwritable("publication failed".into()),
                    None,
                    cleanup,
                    Box::new(SignallingLease(lease_dropped)),
                ))),
            })
        }
    }
}

fn assert_terminal_operation(case: TerminalPublicationCase, operation: Option<&InstallFailure>) {
    match case {
        TerminalPublicationCase::Installed | TerminalPublicationCase::Replaced => {
            assert_eq!(operation, None);
        }
        TerminalPublicationCase::RolledBack => assert!(matches!(
            operation,
            Some(InstallFailure::Store(StoreFailure::Unwritable(detail)))
                if detail == "publication failed"
        )),
        TerminalPublicationCase::RecoveryIncomplete => {
            let Some(InstallFailure::Recovery { operation, cleanup }) = operation else {
                panic!("expected retained recovery operation, got {operation:?}");
            };
            assert_eq!(
                operation,
                &StoreFailure::Unwritable("publication failed".into())
            );
            assert_eq!(cleanup.withdrawal(), None);
            assert_eq!(cleanup.restoration(), None);
            assert_eq!(
                cleanup.confirmation(),
                Some(&StoreFailure::MalformedArchive(
                    "remaining runtime could not be confirmed".into()
                ))
            );
        }
    }
}

#[test]
fn terminal_publication_attempts_outcome_retention_and_audit_independently() {
    for case in [
        TerminalPublicationCase::Installed,
        TerminalPublicationCase::Replaced,
        TerminalPublicationCase::RolledBack,
        TerminalPublicationCase::RecoveryIncomplete,
    ] {
        for (retain_fails, audit_fails) in
            [(true, false), (false, true), (true, true), (false, false)]
        {
            let root = tempfile::tempdir().unwrap();
            let (lease_dropped, dropped) = mpsc::channel();
            let dropped = Arc::new(Mutex::new(dropped));
            let delivery = ScriptedDelivery::new(
                0,
                if retain_fails { 1 } else { 0 },
                0,
                Some(Arc::clone(&dropped)),
            );
            let audit = ScriptedAudit::new(audit_fails, false, Some(Arc::clone(&dropped)));
            let pinned = release("1.18.31", PINNED_DIGEST, &platform());
            let target = RuntimeArtifact::for_release(&pinned);
            let previous =
                RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
            let store = terminal_store(case, root.path(), &previous, lease_dropped);
            let request = request();
            let result = InstallAgentRuntime {
                source: &FakeSource::serving(b"archive bytes"),
                store: store.as_ref(),
                audit: &audit,
                delivery: &delivery,
            }
            .execute(&agent(), &pinned, &host(), &request);

            let terminal = audit.terminal_records().pop().unwrap();
            assert_terminal_case(case, &terminal, &request, &target, &previous);
            assert_eq!(
                delivery
                    .calls()
                    .iter()
                    .filter(|call| matches!(call, DeliveryCall::Retain(_)))
                    .count(),
                1,
                "{case:?}: retention was not attempted exactly once"
            );
            assert_eq!(
                audit.terminal_records().len(),
                1,
                "{case:?}: audit was not attempted exactly once"
            );
            assert!(matches!(dropped.lock().unwrap().try_recv(), Ok(())));

            if retain_fails || audit_fails {
                let InstallFailure::Delivery(failure) = result.unwrap_err() else {
                    panic!("{case:?}: expected combined delivery failure");
                };
                assert_eq!(failure.runtime_state(), &case.expected_runtime_state());
                assert_terminal_case(
                    case,
                    failure.terminal().unwrap(),
                    &request,
                    &target,
                    &previous,
                );
                assert_terminal_operation(case, failure.operation());
                assert_eq!(
                    failure.delivery().map(InstallDeliveryFailure::stage),
                    retain_fails.then_some(InstallDeliveryFailureStage::RetainOutcome)
                );
                assert_eq!(
                    failure.audit().map(AuditFailure::stage),
                    audit_fails.then_some(AuditFailureStage::AcknowledgeRecord)
                );
                assert_eq!(
                    failure.delivery().map(InstallDeliveryFailure::detail),
                    retain_fails.then_some("injected outcome retention failure")
                );
                assert_eq!(
                    failure.audit().map(AuditFailure::detail),
                    audit_fails.then_some("injected terminal audit failure")
                );
                assert!(
                    matches!(
                        delivery.pending(),
                        Some(PendingInstallationDelivery::Prepared(_))
                            if retain_fails
                    ) || matches!(
                        delivery.pending(),
                        Some(PendingInstallationDelivery::Outcome { .. })
                            if !retain_fails
                    )
                );
            } else {
                match case {
                    TerminalPublicationCase::Installed | TerminalPublicationCase::Replaced => {
                        result.unwrap();
                    }
                    TerminalPublicationCase::RolledBack => {
                        assert!(matches!(result, Err(InstallFailure::Store(_))));
                    }
                    TerminalPublicationCase::RecoveryIncomplete => {
                        assert!(matches!(result, Err(InstallFailure::Recovery { .. })));
                    }
                }
                assert!(delivery.pending().is_none());
            }
        }
    }
}

#[test]
fn combined_terminal_failures_block_a_fresh_request_before_install_effects() {
    let root = tempfile::tempdir().unwrap();
    let (lease_dropped, dropped) = mpsc::channel();
    let dropped = Arc::new(Mutex::new(dropped));
    let delivery = ScriptedDelivery::new(0, 2, 0, Some(Arc::clone(&dropped)));
    let audit = ScriptedAudit::new(true, false, Some(Arc::clone(&dropped)));
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path()).signalling_lease_drop(lease_dropped);
    let install = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: &delivery,
    };
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());

    let first = install
        .execute(&agent(), &pinned, &host(), &request())
        .unwrap_err();
    assert!(matches!(first, InstallFailure::Delivery(_)));
    assert!(matches!(
        delivery.pending(),
        Some(PendingInstallationDelivery::Prepared(_))
    ));
    let second_request = InstallRequest::new("unix:501", "install-request-2").unwrap();
    let second = install
        .execute(&agent(), &pinned, &host(), &second_request)
        .unwrap_err();
    let InstallFailure::Delivery(second) = second else {
        panic!("prepared recovery must fail before a new attempt starts");
    };
    assert_eq!(
        second.delivery().unwrap().stage(),
        InstallDeliveryFailureStage::RetainOutcome
    );
    assert_eq!(
        second.audit().unwrap().stage(),
        AuditFailureStage::AcknowledgeRecord
    );
    assert_eq!(source.requested().len(), 1);
    assert_eq!(store.published().len(), 1);
    assert_eq!(store.discarded().len(), 1);
    assert!(audit
        .records()
        .iter()
        .all(|record| record.request().request_id() != "install-request-2"));
}

#[test]
fn settlement_failure_recovers_by_audit_without_repeating_publication() {
    let root = tempfile::tempdir().unwrap();
    let (lease_dropped, dropped) = mpsc::channel();
    let dropped = Arc::new(Mutex::new(dropped));
    let delivery = ScriptedDelivery::new(0, 0, 1, Some(Arc::clone(&dropped)));
    let audit = ScriptedAudit::new(false, true, Some(Arc::clone(&dropped)));
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path()).signalling_lease_drop(lease_dropped);
    let install = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: &delivery,
    };
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());

    let failure = install
        .execute(&agent(), &pinned, &host(), &request())
        .unwrap_err();
    assert!(matches!(
        failure,
        InstallFailure::Delivery(ref evidence)
            if evidence.delivery().unwrap().stage() == InstallDeliveryFailureStage::Settle
    ));
    let next_request = InstallRequest::new("unix:501", "install-request-2").unwrap();
    let next = install
        .execute(&agent(), &pinned, &host(), &next_request)
        .unwrap_err();

    assert_eq!(next, InstallFailure::AttemptReused(next_request));
    assert_eq!(source.requested().len(), 1);
    assert_eq!(store.published().len(), 1);
    assert_eq!(store.discarded().len(), 1);
    assert_eq!(audit.terminal_records().len(), 2);
    assert_eq!(
        delivery
            .calls()
            .iter()
            .filter(|call| matches!(call, DeliveryCall::Retain(_)))
            .count(),
        1
    );
    assert_eq!(
        delivery
            .calls()
            .iter()
            .filter(|call| matches!(call, DeliveryCall::Settle(_)))
            .count(),
        2
    );
    assert!(delivery.pending().is_none());
}

#[test]
fn no_effect_outcomes_are_explicit_for_reuse_and_prepublication_failure() {
    for publication_fails in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let expected_failure =
            publication_fails.then(|| StoreFailure::Unwritable("publication did not begin".into()));
        let store: Box<dyn RuntimeStore> = if let Some(failure) = &expected_failure {
            Box::new(FakeStore::empty(root.path()).failing_to_publish(failure.clone()))
        } else {
            Box::new(FakeStore::empty(root.path()).reusing())
        };
        let delivery = ScriptedDelivery::new(0, 0, 0, None);
        let audit = ScriptedAudit::new(false, false, None);
        let result = InstallAgentRuntime {
            source: &FakeSource::serving(b"archive bytes"),
            store: store.as_ref(),
            audit: &audit,
            delivery: &delivery,
        }
        .execute(
            &agent(),
            &release("1.18.31", PINNED_DIGEST, &platform()),
            &host(),
            &request(),
        );

        match expected_failure {
            Some(expected) => assert_eq!(result.unwrap_err(), InstallFailure::Store(expected)),
            None => {
                result.unwrap();
            }
        }
        let calls = delivery.calls();
        assert!(matches!(
            calls
                .iter()
                .find(|call| matches!(call, DeliveryCall::Retain(_))),
            Some(DeliveryCall::Retain(
                PublicationOutcome::NoPublicationEffect(_)
            ))
        ));
        assert!(audit.terminal_records().is_empty());
        assert!(delivery.pending().is_none());
    }
}

#[test]
fn preparation_failure_discards_staging_without_publishing() {
    let root = tempfile::tempdir().unwrap();
    let delivery = ScriptedDelivery::new(1, 0, 0, None);
    let store = FakeStore::empty(root.path());
    let failure = InstallAgentRuntime {
        source: &FakeSource::serving(b"archive bytes"),
        store: &store,
        audit: audit(),
        delivery: &delivery,
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Delivery(ref evidence)
            if evidence.delivery().unwrap().stage() == InstallDeliveryFailureStage::Prepare
    ));
    assert!(store.published().is_empty());
    assert_eq!(store.discarded().len(), 1);
    assert!(delivery.pending().is_none());
}

#[test]
fn prepared_without_an_exact_outcome_is_not_inferred_as_no_effect() {
    let root = tempfile::tempdir().unwrap();
    let delivery = ScriptedDelivery::new(0, 0, 0, None);
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());
    let original_request = request();
    let (mut attempt, _) = InstallAttempt::start(
        agent(),
        RuntimeArtifact::for_release(&pinned),
        original_request.clone(),
    );
    let preparation = PublicationPreparation::new(attempt.verified().unwrap()).unwrap();
    delivery
        .session(original_request.account_id())
        .unwrap()
        .prepare(preparation.clone())
        .unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let next_request = InstallRequest::new("unix:501", "install-request-2").unwrap();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: &delivery,
    }
    .execute(&agent(), &pinned, &host(), &next_request)
    .unwrap_err();

    assert_eq!(failure, InstallFailure::UnresolvedPublication(preparation));
    assert!(source.requested().is_empty());
    assert!(store.published().is_empty());
    assert!(store.discarded().is_empty());
    assert!(matches!(
        delivery.pending(),
        Some(PendingInstallationDelivery::Prepared(_))
    ));
}

#[test]
fn contradictory_store_result_leaves_preparation_unresolved() {
    let root = tempfile::tempdir().unwrap();
    let delivery = ScriptedDelivery::new(0, 0, 0, None);
    let audit = ScriptedAudit::new(false, false, None);
    let source = FakeSource::serving(b"archive bytes");
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());
    let target = RuntimeArtifact::for_release(&pinned);
    let store = FakeStore::empty(root.path()).replacing(target);
    let install = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: &delivery,
    };

    let first = install
        .execute(&agent(), &pinned, &host(), &request())
        .unwrap_err();
    assert_eq!(
        first,
        InstallFailure::Evidence(InstallAttemptError::Contradictory(
            InstallTransitionError::TargetReportedRestored
        ))
    );
    let preparation = match delivery.pending().unwrap() {
        PendingInstallationDelivery::Prepared(prepared) => prepared.preparation().clone(),
        PendingInstallationDelivery::Outcome { .. } => {
            panic!("a contradictory store result must not be retained as an outcome")
        }
    };
    let next_request = InstallRequest::new("unix:501", "install-request-2").unwrap();
    let second = install
        .execute(&agent(), &pinned, &host(), &next_request)
        .unwrap_err();
    assert_eq!(second, InstallFailure::UnresolvedPublication(preparation));
    assert_eq!(source.requested().len(), 1);
    assert_eq!(store.published().len(), 1);
    assert_eq!(store.discarded().len(), 1);
}

impl RuntimeStore for OneShotFailureStore {
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure> {
        self.inner.installed(agent, release)
    }

    fn stage(&self, agent: &AgentName) -> Result<StagedArchive, StoreFailure> {
        self.inner.stage(agent)
    }

    fn digest(&self, staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure> {
        self.inner.digest(staged)
    }

    fn publish(
        &self,
        _agent: &AgentName,
        _release: &PinnedRelease,
        _staged: &mut StagedArchive,
    ) -> Result<Publication, PublishFailure> {
        Err(self.failure.lock().unwrap().take().unwrap())
    }

    fn discard(&self, staged: StagedArchive) {
        self.inner.discard(staged);
    }
}

fn store_failure_detail(failure: &StoreFailure) -> &str {
    match failure {
        StoreFailure::Unwritable(detail)
        | StoreFailure::Unreadable(detail)
        | StoreFailure::IncompleteArchive(detail)
        | StoreFailure::MalformedArchive(detail) => detail,
    }
}

fn assert_incomplete_transition(transition: &InstallTransition) {
    assert_eq!(transition.kind(), InstallTransitionKind::RecoveryIncomplete);
    let (state, evidence) = transition.recovery().unwrap();
    assert_eq!(state, &RecoveryState::Unconfirmed);
    for retained in [
        evidence.publication(),
        evidence.withdrawal().unwrap(),
        evidence.restoration().unwrap(),
        evidence.confirmation().unwrap(),
    ] {
        assert_eq!(
            retained.detail().len(),
            InstallFailureEvidence::MAX_DETAIL_BYTES
        );
        assert!(retained.truncated());
    }
    assert_eq!(
        evidence.publication().kind(),
        InstallFailureKind::Unwritable
    );
    assert_eq!(
        evidence.withdrawal().unwrap().kind(),
        InstallFailureKind::Unwritable
    );
    assert_eq!(
        evidence.restoration().unwrap().kind(),
        InstallFailureKind::Unreadable
    );
    assert_eq!(
        evidence.confirmation().unwrap().kind(),
        InstallFailureKind::MalformedArchive
    );
}

fn journal_identities(root: &Path) -> Vec<(u64, String, String)> {
    let mut records = std::fs::read_dir(root.join("audit"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    records.sort();
    records
        .iter()
        .map(|path| {
            let record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            (
                record["sequence"].as_u64().unwrap(),
                record["event"]["requestId"].as_str().unwrap().to_owned(),
                record["event"]["slot"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

impl InstallAudit for CommitTerminalThenFailOnceAudit {
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        let terminal = transition.kind() == InstallTransitionKind::Installed;
        let acknowledgement = self.durable.record(transition)?;
        let mut failed = self.failed.lock().unwrap();
        if terminal && !*failed {
            *failed = true;
            return Err(AuditFailure::new(
                AuditFailureStage::AcknowledgeRecord,
                "injected failure after durable commit".into(),
                None,
                None,
            ));
        }
        Ok(acknowledgement)
    }

    fn completion_for(
        &self,
        preparation: &PublicationPreparation,
    ) -> Result<Option<InstallTransition>, AuditFailure> {
        self.durable.completion_for(preparation)
    }
}

impl InstallAudit for FailOnceAudit {
    fn record(&self, _transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        let mut failed = self.0.lock().unwrap();
        if !*failed {
            *failed = true;
            return Err(AuditFailure::new(
                AuditFailureStage::AcknowledgeRecord,
                "uncertain acknowledgement".into(),
                None,
                None,
            ));
        }
        Ok(AuditAcknowledgement::Recorded)
    }
}

impl InstallAudit for BlockingAudit {
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure> {
        if transition.kind() == self.target {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            if self.fail {
                return Err(AuditFailure::new(
                    AuditFailureStage::AcknowledgeRecord,
                    "sink refused".into(),
                    None,
                    None,
                ));
            }
        }
        Ok(AuditAcknowledgement::Recorded)
    }
}

#[test]
fn a_matching_archive_is_published() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();
    let release = release("1.18.31", PINNED_DIGEST, &platform);

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(&agent(), &release, &host, &request())
    .expect("a matching archive installs");

    assert_eq!(installed.version.as_str(), "1.18.31");
    assert!(installed.downloaded);
    assert_eq!(store.published(), vec!["opencode".to_string()]);
    assert_eq!(source.bounded_by(), vec![release.archive_size().bytes()]);
}

#[test]
fn a_mismatched_archive_is_never_unpacked() {
    // The whole point of the pin. A download that is not the tested one must
    // not have its contents read, let alone placed somewhere Nessa will launch
    // from — so `publish` must not have been called at all.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"someone else's bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("a mismatched archive is refused");

    match failure {
        InstallFailure::Rejected(rejection) => {
            assert_eq!(rejection.expected().as_str(), PINNED_DIGEST);
            assert_eq!(rejection.actual().as_str(), OTHER_DIGEST);
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(
        store.published().is_empty(),
        "nothing may be published from an archive that was not the pinned one"
    );
}

#[test]
fn a_rejected_archive_is_discarded() {
    // A refused download is the largest thing this operation writes. Leaving it
    // behind would let a run of failures fill the disk.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"someone else's bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let platform = platform();
    let host = host();

    let _ = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    );

    assert_eq!(store.discarded().len(), 1, "the archive is discarded");
}

#[test]
fn a_successful_install_discards_its_archive_too() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a matching archive installs");

    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn installing_what_is_already_installed_downloads_nothing() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::holding(root.path());
    let platform = platform();
    let host = host();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("an installed runtime is reported as installed");

    assert!(!installed.downloaded);
    assert!(
        source.requested().is_empty(),
        "a runtime already at the pinned version is not fetched again"
    );
    assert!(store.published().is_empty());
}

#[test]
fn a_release_the_store_does_not_hold_is_downloaded() {
    // The store answers about one release, so "nothing installed" here covers
    // an empty machine and one holding a different version alike. Either way
    // the pinned release is fetched.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a newly pinned version installs over an older one");

    assert!(installed.downloaded);
    assert_eq!(installed.version.as_str(), "1.18.31");
    assert_eq!(source.requested().len(), 1);
}

#[test]
fn what_is_measured_is_what_is_unpacked() {
    // The reason the three middle steps share one open file. A store that was
    // handed a path could be measuring one file and unpacking another, and
    // every assertion about ordering above would still pass.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect("a matching archive installs");

    assert_eq!(store.measured(), vec![b"archive bytes".to_vec()]);
    assert_eq!(store.unpacked(), store.measured());
}

#[test]
fn a_store_that_cannot_stage_a_download_fails_before_fetching() {
    // No file to download into is not a network problem, and asking for a
    // hundred megabytes with nowhere to put them helps nobody.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let unwritable = StoreFailure::Unwritable("no room".into());
    let store = FakeStore::empty(root.path()).failing_to_stage(unwritable.clone());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("a store with nowhere to stage fails the install");

    assert_eq!(failure, InstallFailure::Store(unwritable));
    assert!(source.requested().is_empty());
    assert!(
        store.discarded().is_empty(),
        "there is nothing to discard when nothing was staged"
    );
}

#[test]
fn a_release_for_another_platform_is_refused_before_anything_is_fetched() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let elsewhere = host_of(
        &ReleasePlatform::new("linux", "x86_64").expect("usable platform"),
        Some(Libc::Gnu),
        true,
    );

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &elsewhere,
        &request(),
    )
    .expect_err("a release for another platform is refused");

    assert_eq!(failure, InstallFailure::UnsupportedPlatform(elsewhere));
    assert!(source.requested().is_empty());
}

#[test]
fn a_download_failure_is_reported_as_one() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::failing(SourceFailure::Refused(404));
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("a refused download fails the install");

    assert_eq!(
        failure,
        InstallFailure::Download(SourceFailure::Refused(404))
    );
    assert!(store.published().is_empty());
}

#[test]
fn an_archive_without_the_pinned_executable_fails_as_a_store_problem() {
    // A verified archive that does not contain what the pin says it does is a
    // fault in the pin, and it must not be reported as a corrupted download —
    // retrying that would never help.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let missing = StoreFailure::IncompleteArchive("package/bin/opencode".into());
    let store = FakeStore::empty(root.path()).failing_to_publish(missing.clone());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("an archive missing its executable fails the install");

    assert_eq!(failure, InstallFailure::Store(missing));
}

#[test]
fn a_store_that_cannot_say_what_is_installed_does_not_download() {
    // "Could not tell" is not "nothing is installed". Downloading on the
    // strength of an unanswered question would replace a working runtime
    // because a directory happened to be unreadable for a moment.
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let unreadable = StoreFailure::Unreadable("runtime directory".into());
    let store = FakeStore::empty(root.path()).failing_to_read(unreadable.clone());
    let platform = platform();
    let host = host();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform),
        &host,
        &request(),
    )
    .expect_err("an unreadable store fails the install");

    assert_eq!(failure, InstallFailure::Store(unreadable));
    assert!(source.requested().is_empty());
}

#[test]
fn the_pinned_url_is_what_gets_fetched() {
    let root = tempfile::tempdir().expect("temporary root");
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let platform = platform();
    let host = host();
    let release = release("1.18.31", PINNED_DIGEST, &platform);

    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(&agent(), &release, &host, &request())
    .expect("a matching archive installs");

    assert_eq!(
        source.requested(),
        vec![release.archive_url().as_str().to_string()]
    );
}

#[test]
fn the_fake_store_stages_the_way_the_real_one_does() {
    // A test double is only as good as the part of the port it models. The real
    // store creates a name of its own each time so that two installs at once
    // cannot truncate each other's download, and `StagedArchive` makes that a
    // promise of the port — so a fake that staged over one fixed path would let
    // a test assert an ordering guarantee while quietly modelling the thing the
    // guarantee exists to prevent.
    let root = tempfile::tempdir().expect("temporary root");
    let store = FakeStore::empty(root.path());

    let first = store.stage(&agent()).expect("a staged file");
    let second = store.stage(&agent()).expect("a second staged file");

    assert_ne!(
        first.path(),
        second.path(),
        "two downloads shared one staged file"
    );
}

#[test]
fn a_build_this_machine_cannot_run_is_refused_before_anything_is_fetched() {
    // The other half of the refusal, and the one this change introduced: the
    // platform matches and the *requirements* do not. A musl build on a glibc
    // machine dies in the loader, and an AVX2 build on a processor without it
    // dies on an illegal instruction, so neither is worth a hundred megabytes
    // first.
    //
    // The pre-existing half — a release for another operating system — is
    // covered above; the old code caught that one, and only that one.
    //
    // Linux x86-64 rather than the suite's usual macOS, because both scenarios
    // below need a platform where they could actually happen: macOS has one C
    // library and aarch64 has no AVX2, so a release making either demand there
    // is one `PinnedRelease::new` refuses to assemble at all.
    let platform = ReleasePlatform::new("linux", "x86_64").expect("usable platform");
    for (named, host, requirements) in [
        (
            "a musl build on a glibc machine",
            host_of(&platform, Some(Libc::Gnu), true),
            ReleaseRequirements::new(Some(Libc::Musl), false),
        ),
        (
            "a glibc build on a machine with neither",
            host_of(&platform, None, true),
            ReleaseRequirements::new(Some(Libc::Gnu), false),
        ),
        (
            "an avx2 build on a processor without it",
            host_of(&platform, Some(Libc::Gnu), false),
            ReleaseRequirements::new(Some(Libc::Gnu), true),
        ),
    ] {
        let root = tempfile::tempdir().expect("temporary root");
        let source = FakeSource::serving(b"archive bytes");
        let store = FakeStore::empty(root.path());

        let failure = InstallAgentRuntime {
            source: &source,
            store: &store,
            audit: audit(),
            delivery: delivery(),
        }
        .execute(
            &agent(),
            &release_needing("1.18.31", PINNED_DIGEST, &platform, requirements),
            &host,
            &request(),
        )
        .expect_err(named);

        assert_eq!(failure, InstallFailure::UnsupportedPlatform(host));
        assert!(
            source.requested().is_empty(),
            "{named} was downloaded before it was refused"
        );
    }
}

#[test]
fn a_new_install_records_one_correlated_legal_sequence() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::default();
    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap();

    let records = audit.records();
    assert_eq!(
        records
            .iter()
            .map(InstallTransition::kind)
            .collect::<Vec<_>>(),
        [
            InstallTransitionKind::Started,
            InstallTransitionKind::Verified,
            InstallTransitionKind::Installed,
        ]
    );
    assert!(records
        .iter()
        .all(|record| record.request().request_id() == "install-request-1"));
}

#[test]
fn replacement_evidence_uses_the_artifact_seen_under_the_publication_lock() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let store = FakeStore::empty(root.path()).replacing(previous.clone());
    let audit = RecordingAudit::default();
    InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap();

    let records = audit.records();
    let replaced = records.last().unwrap();
    assert_eq!(replaced.kind(), InstallTransitionKind::Replaced);
    assert_eq!(replaced.previous(), Some(&previous));
}

#[test]
fn digest_rejection_is_audited_and_the_archive_is_discarded() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"other bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let audit = RecordingAudit::default();
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(failure, InstallFailure::Rejected(_)));
    assert_eq!(
        audit.records().last().unwrap().kind(),
        InstallTransitionKind::DigestRejected
    );
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn store_rollback_is_audited_without_hiding_the_store_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store_failure = StoreFailure::Unwritable("directory sync failed".into());
    let store = FakeStore::empty(root.path())
        .failing_after_rollback(store_failure.clone(), RollbackChange::NoInstalledRuntime);
    let audit = RecordingAudit::default();
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert_eq!(failure, InstallFailure::Store(store_failure));
    assert_eq!(
        audit.records().last().unwrap().kind(),
        InstallTransitionKind::RolledBack
    );
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn audit_failure_after_verification_is_visible_and_prevents_publication() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Verified, "sink refused");
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::Unchanged
    ));
    assert!(store.published().is_empty());
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn audit_failure_at_started_prevents_install_effects() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Started, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::Unchanged
    ));
    assert!(source.requested().is_empty());
    assert!(store.published().is_empty());
}

#[test]
fn replayed_start_refuses_request_reexecution_before_any_install_effect() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::replaying();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(failure, InstallFailure::AttemptReused(_)));
    assert!(source.requested().is_empty());
    assert!(store.published().is_empty());
}

#[test]
fn retained_pending_transition_can_be_redelivered_without_repeating_install_effects() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = FailOnceAudit(Mutex::new(false));
    let install = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    };
    let failure = install
        .execute(
            &agent(),
            &release("1.18.31", PINNED_DIGEST, &platform()),
            &host(),
            &request(),
        )
        .unwrap_err();

    assert_eq!(
        install.retry_audit(&failure),
        Ok(AuditAcknowledgement::Recorded)
    );
    assert!(source.requested().is_empty());
    assert!(store.published().is_empty());
}

#[test]
fn incomplete_publication_moves_full_diagnostics_and_bounds_only_audit_evidence() {
    for sink_fails in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (lease_dropped, dropped) = mpsc::channel();
        let publication = StoreFailure::Unwritable("p".repeat(5_000));
        let withdrawal = StoreFailure::Unwritable("w".repeat(5_000));
        let restoration = StoreFailure::Unreadable("r".repeat(5_000));
        let confirmation = StoreFailure::MalformedArchive("c".repeat(5_000));
        let pointers = [
            store_failure_detail(&publication).as_ptr(),
            store_failure_detail(&withdrawal).as_ptr(),
            store_failure_detail(&restoration).as_ptr(),
            store_failure_detail(&confirmation).as_ptr(),
        ];
        let cleanup =
            PublicationCleanupFailure::new(Some(withdrawal), Some(restoration), Some(confirmation))
                .unwrap();
        let store = OneShotFailureStore {
            inner: FakeStore::empty(root.path()),
            failure: Mutex::new(Some(PublishFailure::incomplete(
                publication,
                None,
                cleanup,
                Box::new(SignallingLease(lease_dropped)),
            ))),
        };
        let audit = LeaseCheckingAudit {
            target: InstallTransitionKind::RecoveryIncomplete,
            dropped: Mutex::new(dropped),
            recorded: Mutex::new(Vec::new()),
            fail: sink_fails,
        };

        let failure = InstallAgentRuntime {
            source: &FakeSource::serving(b"archive bytes"),
            store: &store,
            audit: &audit,
            delivery: delivery(),
        }
        .execute(
            &agent(),
            &release("1.18.31", PINNED_DIGEST, &platform()),
            &host(),
            &request(),
        )
        .unwrap_err();

        let operation_failure = match &failure {
            InstallFailure::Audit(delivery) if sink_fails => {
                assert_eq!(delivery.runtime_state(), &RuntimeStateEvidence::Unconfirmed);
                assert_incomplete_transition(delivery.pending());
                delivery.operation().unwrap()
            }
            InstallFailure::Recovery { .. } if !sink_fails => &failure,
            _ => panic!("unexpected incomplete recovery result: {failure:?}"),
        };
        let InstallFailure::Recovery { operation, cleanup } = operation_failure else {
            panic!("expected retained recovery failure: {operation_failure:?}");
        };
        assert_eq!(store_failure_detail(operation).as_ptr(), pointers[0]);
        assert_eq!(
            store_failure_detail(cleanup.withdrawal().unwrap()).as_ptr(),
            pointers[1]
        );
        assert_eq!(
            store_failure_detail(cleanup.restoration().unwrap()).as_ptr(),
            pointers[2]
        );
        assert_eq!(
            store_failure_detail(cleanup.confirmation().unwrap()).as_ptr(),
            pointers[3]
        );
        let recorded = audit.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_incomplete_transition(&recorded[0]);
        assert!(matches!(audit.dropped.lock().unwrap().try_recv(), Ok(())));
    }
}

#[test]
fn publication_failures_move_original_store_error_and_hold_each_lease_scope() {
    {
        let root = tempfile::tempdir().unwrap();
        let (lease_dropped, dropped) = mpsc::channel();
        let operation = StoreFailure::Unwritable("n".repeat(5_000));
        let pointer = store_failure_detail(&operation).as_ptr();
        let store = OneShotFailureStore {
            inner: FakeStore::empty(root.path()),
            failure: Mutex::new(Some(PublishFailure::unchanged(
                operation,
                Box::new(SignallingLease(lease_dropped)),
            ))),
        };
        let audit = RecordingAudit::default();
        let failure = InstallAgentRuntime {
            source: &FakeSource::serving(b"archive bytes"),
            store: &store,
            audit: &audit,
            delivery: delivery(),
        }
        .execute(
            &agent(),
            &release("1.18.31", PINNED_DIGEST, &platform()),
            &host(),
            &request(),
        )
        .unwrap_err();
        let InstallFailure::Store(operation) = failure else {
            panic!("expected original unchanged store failure: {failure:?}");
        };
        assert_eq!(store_failure_detail(&operation).as_ptr(), pointer);
        assert_eq!(
            audit
                .records()
                .iter()
                .map(InstallTransition::kind)
                .collect::<Vec<_>>(),
            vec![
                InstallTransitionKind::Started,
                InstallTransitionKind::Verified,
            ]
        );
        assert_eq!(dropped.try_recv(), Ok(()));
    }

    for sink_fails in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (lease_dropped, dropped) = mpsc::channel();
        let operation = StoreFailure::Unwritable("r".repeat(5_000));
        let pointer = store_failure_detail(&operation).as_ptr();
        let store = OneShotFailureStore {
            inner: FakeStore::empty(root.path()),
            failure: Mutex::new(Some(PublishFailure::rolled_back(
                operation,
                RollbackChange::NoInstalledRuntime,
                Box::new(SignallingLease(lease_dropped)),
            ))),
        };
        let audit = LeaseCheckingAudit {
            target: InstallTransitionKind::RolledBack,
            dropped: Mutex::new(dropped),
            recorded: Mutex::new(Vec::new()),
            fail: sink_fails,
        };

        let failure = InstallAgentRuntime {
            source: &FakeSource::serving(b"archive bytes"),
            store: &store,
            audit: &audit,
            delivery: delivery(),
        }
        .execute(
            &agent(),
            &release("1.18.31", PINNED_DIGEST, &platform()),
            &host(),
            &request(),
        )
        .unwrap_err();

        let operation_failure = match &failure {
            InstallFailure::Audit(delivery) if sink_fails => {
                assert_eq!(
                    delivery.runtime_state(),
                    &RuntimeStateEvidence::NoInstalledRuntime
                );
                assert_eq!(
                    delivery.pending().rollback(),
                    Some(&RollbackState::NoInstalledRuntime)
                );
                delivery.operation().unwrap()
            }
            InstallFailure::Store(_) if !sink_fails => &failure,
            _ => panic!("unexpected rolled-back result: {failure:?}"),
        };
        let InstallFailure::Store(operation) = operation_failure else {
            panic!("expected retained store failure: {operation_failure:?}");
        };
        assert_eq!(store_failure_detail(operation).as_ptr(), pointer);
        let recorded = audit.recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].rollback(),
            Some(&RollbackState::NoInstalledRuntime)
        );
        assert!(matches!(audit.dropped.lock().unwrap().try_recv(), Ok(())));
    }
}

#[test]
fn publication_lease_survives_until_evidence_validation_returns() {
    let root = tempfile::tempdir().unwrap();
    let (lease_dropped, dropped) = mpsc::channel();
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());
    let store = OneShotFailureStore {
        inner: FakeStore::empty(root.path()),
        failure: Mutex::new(Some(PublishFailure::rolled_back(
            StoreFailure::Unwritable("publication".into()),
            RollbackChange::Restored(RuntimeArtifact::for_release(&pinned)),
            Box::new(SignallingLease(lease_dropped)),
        ))),
    };

    let failure = InstallAgentRuntime {
        source: &FakeSource::serving(b"archive bytes"),
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(&agent(), &pinned, &host(), &request())
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Evidence(InstallAttemptError::Contradictory(
            InstallTransitionError::TargetReportedRestored
        ))
    ));
    assert_eq!(dropped.try_recv(), Ok(()));
}

#[test]
fn postcommit_terminal_failure_replays_without_changing_later_journal_order() {
    let store_root = tempfile::tempdir().unwrap();
    let audit_root = temporary_root();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(store_root.path());
    let audit = CommitTerminalThenFailOnceAudit {
        durable: DurableInstallAudit::new(
            audit_root.path(),
            Path::new("audit"),
            Arc::new(FixedClock),
        )
        .unwrap(),
        failed: Mutex::new(false),
    };
    let delivery = DurableInstallationDelivery::new(
        audit_root.path(),
        Path::new("delivery"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let install = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: &delivery,
    };
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());
    let earlier = InstallRequest::new("unix:501", "earlier").unwrap();
    let failure = install
        .execute(&agent(), &pinned, &host(), &earlier)
        .unwrap_err();
    let later = InstallRequest::new("unix:501", "later").unwrap();
    install.execute(&agent(), &pinned, &host(), &later).unwrap();
    assert!(matches!(failure, InstallFailure::Delivery(_)));

    assert_eq!(
        journal_identities(audit_root.path()),
        vec![
            (1, "earlier".into(), "started".into()),
            (2, "earlier".into(), "verification_outcome".into()),
            (3, "earlier".into(), "completion_outcome".into()),
            (4, "later".into(), "started".into()),
            (5, "later".into(), "verification_outcome".into()),
            (6, "later".into(), "completion_outcome".into()),
        ]
    );
}

#[test]
fn precommit_terminal_failure_is_recovered_before_a_later_install() {
    let store_root = tempfile::tempdir().unwrap();
    let audit_root = temporary_root();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(store_root.path());
    let audit = RefuseTerminalBeforeCommitOnceAudit {
        durable: DurableInstallAudit::new(
            audit_root.path(),
            Path::new("audit"),
            Arc::new(FixedClock),
        )
        .unwrap(),
        failed: Mutex::new(false),
    };
    let delivery = DurableInstallationDelivery::new(
        audit_root.path(),
        Path::new("delivery"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let install = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: &delivery,
    };
    let pinned = release("1.18.31", PINNED_DIGEST, &platform());
    let earlier = InstallRequest::new("unix:501", "earlier").unwrap();
    let failure = install
        .execute(&agent(), &pinned, &host(), &earlier)
        .unwrap_err();
    let later = InstallRequest::new("unix:501", "later").unwrap();
    let later_runtime = install.execute(&agent(), &pinned, &host(), &later).unwrap();
    assert_eq!(later_runtime.version, pinned.version().clone());
    assert!(later_runtime.downloaded);
    assert_eq!(later_runtime.executable, store_root.path().join("opencode"));
    assert_eq!(
        journal_identities(audit_root.path()),
        vec![
            (1, "earlier".into(), "started".into()),
            (2, "earlier".into(), "verification_outcome".into()),
            (3, "earlier".into(), "completion_outcome".into()),
            (4, "later".into(), "started".into()),
            (5, "later".into(), "verification_outcome".into()),
            (6, "later".into(), "completion_outcome".into()),
        ]
    );
    assert!(matches!(failure, InstallFailure::Delivery(_)));
    assert_eq!(store.published().len(), 2);
}

#[test]
fn audit_failure_at_replaced_reports_the_new_runtime_and_prior_evidence() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let store = FakeStore::empty(root.path()).replacing(previous.clone());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Replaced, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::TargetInstalled
    ));
    assert_eq!(audit.records().last().unwrap().previous(), Some(&previous));
}

#[test]
fn audit_failure_at_rollback_preserves_the_publication_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let operation = StoreFailure::Unwritable("publish failed".into());
    let store = FakeStore::empty(root.path())
        .failing_after_rollback(operation.clone(), RollbackChange::NoInstalledRuntime);
    let audit = RecordingAudit::failing_on(InstallTransitionKind::RolledBack, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::NoInstalledRuntime
                && evidence.operation() == Some(&InstallFailure::Store(operation))
    ));
}

#[test]
fn cleanup_failure_is_visible_without_replacing_the_publication_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let operation = StoreFailure::Unwritable("record sync failed".into());
    let cleanup = StoreFailure::Unwritable("withdrawal failed".into());
    let cleanup_evidence =
        PublicationCleanupFailure::new(Some(cleanup), None, None).expect("one cleanup failure");
    let store = FakeStore::empty(root.path()).failing_after_incomplete_cleanup(
        operation.clone(),
        Some(RollbackChange::NoInstalledRuntime),
        cleanup_evidence.clone(),
    );
    let audit = RecordingAudit::default();

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert_eq!(
        failure,
        InstallFailure::Recovery {
            operation,
            cleanup: Box::new(cleanup_evidence),
        }
    );
    assert_eq!(
        audit.records().last().unwrap().kind(),
        InstallTransitionKind::RecoveryIncomplete
    );
    let record = audit.records().pop().unwrap();
    let (state, failures) = record.recovery().unwrap();
    assert_eq!(
        state,
        &RecoveryState::Confirmed(RollbackState::NoInstalledRuntime)
    );
    assert_eq!(failures.publication().detail(), "record sync failed");
    assert_eq!(failures.withdrawal().unwrap().detail(), "withdrawal failed");
}

#[test]
fn incomplete_recovery_retains_the_confirmed_prior_artifact() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let previous = RuntimeArtifact::for_release(&release("1.17.0", OTHER_DIGEST, &platform()));
    let operation = StoreFailure::Unwritable("record sync failed".into());
    let cleanup = PublicationCleanupFailure::new(
        None,
        Some(StoreFailure::Unwritable("restoration sync failed".into())),
        None,
    )
    .unwrap();
    let store = FakeStore::empty(root.path()).failing_after_incomplete_cleanup(
        operation,
        Some(RollbackChange::Restored(previous.clone())),
        cleanup,
    );
    let audit =
        RecordingAudit::failing_on(InstallTransitionKind::RecoveryIncomplete, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.operation().is_some()
                && evidence.runtime_state() == &RuntimeStateEvidence::Restored(previous.clone())
    ));
    let transition = audit.records().pop().unwrap();
    assert!(matches!(
        transition.recovery(),
        Some((RecoveryState::Confirmed(RollbackState::Restored(artifact)), _))
            if artifact == &previous
    ));
}

#[test]
fn unconfirmed_recovery_and_audit_failure_retain_every_failure() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let operation = StoreFailure::Unwritable("record sync failed".into());
    let withdrawal = StoreFailure::Unwritable("withdrawal failed".into());
    let restoration = StoreFailure::Unreadable("restoration failed".into());
    let confirmation = StoreFailure::Unreadable("confirmation failed".into());
    let cleanup_evidence = PublicationCleanupFailure::new(
        Some(withdrawal.clone()),
        Some(restoration.clone()),
        Some(confirmation.clone()),
    )
    .unwrap();
    let store = FakeStore::empty(root.path()).failing_after_incomplete_cleanup(
        operation.clone(),
        None,
        cleanup_evidence.clone(),
    );
    let audit =
        RecordingAudit::failing_on(InstallTransitionKind::RecoveryIncomplete, "sink refused");

    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    let InstallFailure::Audit(evidence) = failure else {
        panic!("incomplete recovery and audit failure were not retained");
    };
    assert_eq!(evidence.runtime_state(), &RuntimeStateEvidence::Unconfirmed);
    assert_eq!(evidence.failure().detail(), "sink refused");
    assert_eq!(
        evidence.operation(),
        Some(&InstallFailure::Recovery {
            operation,
            cleanup: Box::new(cleanup_evidence),
        })
    );
    let transition = audit.records().pop().unwrap();
    let (state, failures) = transition.recovery().unwrap();
    assert_eq!(state, &RecoveryState::Unconfirmed);
    assert_eq!(failures.publication().detail(), "record sync failed");
    assert_eq!(failures.withdrawal().unwrap().detail(), "withdrawal failed");
    assert_eq!(
        failures.restoration().unwrap().detail(),
        "restoration failed"
    );
    assert_eq!(
        failures.confirmation().unwrap().detail(),
        "confirmation failed"
    );
}

#[test]
fn audit_failure_after_publication_reports_the_runtime_as_installed() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path());
    let audit = RecordingAudit::failing_on(InstallTransitionKind::Installed, "sink refused");
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::TargetInstalled
    ));
    assert_eq!(store.published(), ["opencode"]);
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn audit_failure_preserves_digest_rejection_and_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let store = FakeStore::empty(root.path()).hashing(OTHER_DIGEST);
    let audit = RecordingAudit::failing_on(InstallTransitionKind::DigestRejected, "sink refused");
    let failure = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: &audit,
        delivery: delivery(),
    }
    .execute(
        &agent(),
        &release("1.18.31", PINNED_DIGEST, &platform()),
        &host(),
        &request(),
    )
    .unwrap_err();

    assert!(matches!(
        failure,
        InstallFailure::Audit(ref evidence)
            if evidence.runtime_state() == &RuntimeStateEvidence::Unchanged
                && matches!(evidence.operation(), Some(InstallFailure::Rejected(_)))
    ));
    assert!(store.published().is_empty());
    assert_eq!(store.discarded().len(), 1);
}

#[test]
fn successful_publication_lease_spans_a_failing_audit_and_then_releases() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let (lease_dropped, dropped) = mpsc::channel();
    let store = FakeStore::empty(root.path()).signalling_lease_drop(lease_dropped);
    let (entered_send, entered) = mpsc::sync_channel(0);
    let (release, release_recv) = mpsc::sync_channel(0);
    let audit = BlockingAudit {
        target: InstallTransitionKind::Installed,
        entered: entered_send,
        release: Mutex::new(release_recv),
        fail: true,
    };

    std::thread::scope(|threads| {
        let install = threads.spawn(|| {
            InstallAgentRuntime {
                source: &source,
                store: &store,
                audit: &audit,
                delivery: delivery(),
            }
            .execute(
                &agent(),
                &crate::agent_install_test_support::release("1.18.31", PINNED_DIGEST, &platform()),
                &host(),
                &request(),
            )
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(matches!(
            install.join().unwrap(),
            Err(InstallFailure::Audit(ref evidence))
                if evidence.runtime_state() == &RuntimeStateEvidence::TargetInstalled
        ));
        dropped.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}

#[test]
fn rollback_lease_spans_successful_audit_and_then_releases() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let (lease_dropped, dropped) = mpsc::channel();
    let store = FakeStore::empty(root.path())
        .failing_after_rollback(
            StoreFailure::Unwritable("publish failed".into()),
            RollbackChange::NoInstalledRuntime,
        )
        .signalling_lease_drop(lease_dropped);
    let (entered_send, entered) = mpsc::sync_channel(0);
    let (release, release_recv) = mpsc::sync_channel(0);
    let audit = BlockingAudit {
        target: InstallTransitionKind::RolledBack,
        entered: entered_send,
        release: Mutex::new(release_recv),
        fail: false,
    };

    std::thread::scope(|threads| {
        let install = threads.spawn(|| {
            InstallAgentRuntime {
                source: &source,
                store: &store,
                audit: &audit,
                delivery: delivery(),
            }
            .execute(
                &agent(),
                &crate::agent_install_test_support::release("1.18.31", PINNED_DIGEST, &platform()),
                &host(),
                &request(),
            )
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(matches!(
            install.join().unwrap(),
            Err(InstallFailure::Store(_))
        ));
        dropped.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}

#[test]
fn uncertain_recovery_lease_spans_failing_audit_and_then_releases() {
    let root = tempfile::tempdir().unwrap();
    let source = FakeSource::serving(b"archive bytes");
    let cleanup = PublicationCleanupFailure::new(
        Some(StoreFailure::Unwritable("withdrawal failed".into())),
        None,
        Some(StoreFailure::Unreadable("confirmation failed".into())),
    )
    .unwrap();
    let (lease_dropped, dropped) = mpsc::channel();
    let store = FakeStore::empty(root.path())
        .failing_after_incomplete_cleanup(
            StoreFailure::Unwritable("publish failed".into()),
            None,
            cleanup,
        )
        .signalling_lease_drop(lease_dropped);
    let (entered_send, entered) = mpsc::sync_channel(0);
    let (release, release_recv) = mpsc::sync_channel(0);
    let audit = BlockingAudit {
        target: InstallTransitionKind::RecoveryIncomplete,
        entered: entered_send,
        release: Mutex::new(release_recv),
        fail: true,
    };

    std::thread::scope(|threads| {
        let install = threads.spawn(|| {
            InstallAgentRuntime {
                source: &source,
                store: &store,
                audit: &audit,
                delivery: delivery(),
            }
            .execute(
                &agent(),
                &crate::agent_install_test_support::release("1.18.31", PINNED_DIGEST, &platform()),
                &host(),
                &request(),
            )
        });
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(dropped.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert!(matches!(
            install.join().unwrap(),
            Err(InstallFailure::Audit(ref evidence))
                if evidence.operation().is_some()
                    && evidence.runtime_state() == &RuntimeStateEvidence::Unconfirmed
        ));
        dropped.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}
