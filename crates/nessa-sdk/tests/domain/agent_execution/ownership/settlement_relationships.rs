//! Late physical facts and durable close authority preserve their exact relationships.
use super::*;

fn reserved_child() -> OwnershipGraph {
    let mut graph = OwnershipGraph::default();
    assert_eq!(graph.snapshot(), OwnershipGraph::new().snapshot());
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "request");
    graph
}
fn closing_root(operation: &str) -> OwnershipGraph {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "root");
    graph
        .begin_close(
            &life("root"),
            close_id(operation),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    graph
}
fn ready_root(operation: &str) -> OwnershipGraph {
    let mut graph = closing_root(operation);
    let record = graph
        .apply_report(
            &life("root"),
            &close_id(operation),
            &life("root"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    graph
        .acknowledge_observation(&record, EvidenceFact::Acknowledged)
        .unwrap();
    graph
}

#[test]
fn late_prepared_owner_preserves_safety_progress_and_rejects_ended_authority() {
    for progress in [
        SpawnProgress::Draining {
            known: KnownMilestone::Reserved,
        },
        SpawnProgress::Unconfirmed {
            known: KnownMilestone::Reserved,
        },
    ] {
        let mut graph = reserved_child();
        graph
            .advance_spawn(&spawn_id("request"), progress.clone())
            .unwrap();
        graph.note_prepared_owner(&spawn_id("request")).unwrap();
        let expected = match progress {
            SpawnProgress::Draining { .. } => SpawnProgress::Draining {
                known: KnownMilestone::Prepared,
            },
            _ => SpawnProgress::Unconfirmed {
                known: KnownMilestone::Prepared,
            },
        };
        assert_eq!(graph.spawn_progress(&spawn_id("request")), Some(&expected));
        assert!(OwnershipGraph::restore(graph.snapshot())
            .refusal()
            .is_none());
        let before = graph.snapshot();
        assert_eq!(
            graph.note_prepared_owner(&spawn_id("request")),
            Err(OwnershipError::IllegalSpawnProgress)
        );
        assert_eq!(graph.snapshot(), before);
    }
    for ended in [false, true] {
        let mut graph = reserved_child();
        graph
            .advance_spawn(
                &spawn_id("request"),
                SpawnProgress::StartupFailed {
                    known: KnownMilestone::Reserved,
                },
            )
            .unwrap();
        if ended {
            graph
                .advance_spawn(
                    &spawn_id("request"),
                    SpawnProgress::Ended {
                        known: KnownMilestone::Reserved,
                    },
                )
                .unwrap();
        }
        let before = graph.snapshot();
        assert_eq!(
            graph.note_prepared_owner(&spawn_id("request")),
            Err(OwnershipError::StaleOutcome)
        );
        assert_eq!(graph.snapshot(), before);
    }
}

#[test]
fn late_task_receipt_preserves_sealed_progress_and_exact_receipt_identity() {
    let mut attached = reserved_child();
    let before = attached.snapshot();
    assert_eq!(
        attached.note_task_receipt(&spawn_id("request"), receipt("actual")),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    assert_eq!(attached.snapshot(), before);
    attached
        .advance_spawn(&spawn_id("request"), SpawnProgress::Prepared)
        .unwrap();
    attached
        .advance_spawn(&spawn_id("request"), SpawnProgress::Attached)
        .unwrap();
    for stage in 0..3 {
        let mut graph = attached.clone();
        let safety = if stage == 2 {
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Attached,
            }
        } else {
            SpawnProgress::Draining {
                known: KnownMilestone::Attached,
            }
        };
        graph.advance_spawn(&spawn_id("request"), safety).unwrap();
        if stage == 1 {
            graph
                .advance_spawn(
                    &spawn_id("request"),
                    SpawnProgress::Ended {
                        known: KnownMilestone::Attached,
                    },
                )
                .unwrap();
        }
        graph
            .note_task_receipt(&spawn_id("request"), receipt("actual"))
            .unwrap();
        let known = KnownMilestone::TaskAdmitted {
            receipt: receipt("actual"),
        };
        let expected = match stage {
            0 => SpawnProgress::Draining { known },
            1 => SpawnProgress::Ended { known },
            _ => SpawnProgress::Unconfirmed { known },
        };
        assert_eq!(graph.spawn_progress(&spawn_id("request")), Some(&expected));
        let before = graph.snapshot();
        graph
            .note_task_receipt(&spawn_id("request"), receipt("actual"))
            .unwrap();
        assert_eq!(
            graph.snapshot(),
            before,
            "same actual receipt is idempotent"
        );
        assert_eq!(
            graph.note_task_receipt(&spawn_id("request"), receipt("foreign")),
            Err(OwnershipError::StaleOutcome)
        );
        assert_eq!(graph.snapshot(), before);
        assert!(OwnershipGraph::restore(before).refusal().is_none());
    }
}

#[test]
fn absence_replay_keeps_exact_record_and_rejects_conflicting_authority() {
    let mut graph = closing_root("close");
    let token = graph
        .note_unbound_root(&life("root"), &close_id("close"))
        .unwrap();
    let exact = token.evidence().clone();
    let before = graph.snapshot();
    assert_eq!(
        graph
            .note_absence(
                &life("root"),
                &close_id("close"),
                &life("root"),
                AbsenceProof::NeverTransferredRoot
            )
            .unwrap(),
        exact
    );
    assert_eq!(graph.snapshot(), before);
    let mut altered = exact.clone();
    altered.initiator = Initiator::Host(actor("foreign"));
    assert_eq!(
        graph.acknowledge_observation(&altered, EvidenceFact::Acknowledged),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(graph.snapshot(), before);
    let mut different_operation = closing_root("other-close");
    let other_before = different_operation.snapshot();
    assert_eq!(
        different_operation.acknowledge_unbound_root(token, EvidenceFact::Acknowledged),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(different_operation.snapshot(), other_before);
    let mut child = reserved_child();
    child
        .begin_close(
            &life("child"),
            close_id("child-close"),
            LifetimeCause::TerminalFailure,
            Initiator::Runtime,
        )
        .unwrap();
    child
        .note_absence(
            &life("child"),
            &close_id("child-close"),
            &life("child"),
            AbsenceProof::PreparationRejectedWithoutOwner(spawn_id("request")),
        )
        .unwrap();
    let before = child.snapshot();
    assert_eq!(
        child.note_absence(
            &life("child"),
            &close_id("child-close"),
            &life("child"),
            AbsenceProof::AdmissionFailedBeforeFactory(spawn_id("request"))
        ),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(child.snapshot(), before);
    let mut resource = ready_root("close");
    let before = resource.snapshot();
    assert_eq!(
        resource.note_absence(
            &life("root"),
            &close_id("close"),
            &life("root"),
            AbsenceProof::NeverTransferredRoot
        ),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(resource.snapshot(), before);
}

#[test]
fn completion_token_cannot_settle_unready_or_different_close_history() {
    let mut source = ready_root("first-close");
    let token = source
        .prepare_completion(&life("root"), &close_id("first-close"))
        .unwrap();
    let mut unready = OwnershipGraph::new();
    root(&mut unready, "root");
    let before = unready.snapshot();
    assert_eq!(
        unready.acknowledge_completion(&token, EvidenceFact::Acknowledged),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(unready.snapshot(), before);
    let mut other = ready_root("other-close");
    other
        .prepare_completion(&life("root"), &close_id("other-close"))
        .unwrap();
    let before = other.snapshot();
    assert_eq!(
        other.acknowledge_completion(&token, EvidenceFact::Acknowledged),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(other.snapshot(), before);
    source
        .acknowledge_completion(&token, EvidenceFact::Acknowledged)
        .unwrap();
    assert_eq!(
        source.lifetime_state(&life("root")),
        Some(LifetimeState::Closed)
    );
    assert!(OwnershipGraph::restore(source.snapshot())
        .refusal()
        .is_none());
}

#[test]
fn restored_cascade_cannot_invent_open_or_mismatched_first_close_authority() {
    let graph = reserved_child();
    let mut open_cascade = graph.snapshot();
    open_cascade
        .lifetimes
        .iter_mut()
        .find(|row| row.lifetime_id == life("child"))
        .unwrap()
        .cascaded_from = Some(life("parent"));
    let restored = OwnershipGraph::restore(open_cascade.clone());
    assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(restored.snapshot(), open_cascade);
    let mut closed = graph;
    closed
        .begin_close(
            &life("parent"),
            close_id("parent-close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let valid = closed.snapshot();
    assert!(OwnershipGraph::restore(valid.clone()).refusal().is_none());
    let mut wrong_cause = valid;
    wrong_cause
        .lifetimes
        .iter_mut()
        .find(|row| row.lifetime_id == life("child"))
        .unwrap()
        .cause = Some(LifetimeCause::Deletion);
    let mut restored = OwnershipGraph::restore(wrong_cause.clone());
    assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(restored.close_owner(&life("child")), None);
    assert_eq!(restored.snapshot(), wrong_cause);
    let before = restored.snapshot();
    assert_eq!(
        restored.apply_report(
            &life("parent"),
            &close_id("parent-close"),
            &life("child"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged
        ),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(restored.snapshot(), before);
}

#[test]
fn restored_completion_with_empty_resource_proof_is_refused_without_repair() {
    let mut graph = ready_root("close");
    complete(&mut graph, &life("root"));
    let mut history = graph.snapshot();
    assert!(OwnershipGraph::restore(history.clone()).refusal().is_none());
    history.settlements[0].proof = SettlementProof::Resource(Box::new([None, None, None]));
    history.settlements[0].physical = PhysicalFact::Pending;
    history.settlements[0].evidence = EvidenceFact::Pending;
    let restored = OwnershipGraph::restore(history.clone());
    assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(restored.snapshot(), history);
}
