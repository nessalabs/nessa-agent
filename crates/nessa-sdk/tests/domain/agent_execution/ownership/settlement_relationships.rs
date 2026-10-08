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

#[test]
fn refused_restore_blocks_late_handoffs_absence_and_completion_without_changing_history() {
    let mut donor = ready_root("close");
    let token = donor
        .prepare_completion(&life("root"), &close_id("close"))
        .unwrap();
    donor
        .acknowledge_completion(&token, EvidenceFact::Acknowledged)
        .unwrap();
    let mut history = donor.snapshot();
    history.lifetimes[0].close_operation = None;
    let mut refused = OwnershipGraph::restore(history.clone());
    assert_eq!(refused.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(
        refused.note_prepared_owner(&spawn_id("request")),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(
        refused.note_task_receipt(&spawn_id("request"), receipt("actual")),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(
        refused.note_absence(
            &life("root"),
            &close_id("close"),
            &life("root"),
            AbsenceProof::NeverTransferredRoot
        ),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(
        refused
            .prepare_completion(&life("root"), &close_id("close"))
            .unwrap_err(),
        OwnershipError::DispatchRefused
    );
    assert_eq!(
        refused.acknowledge_completion(&token, EvidenceFact::Acknowledged),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(refused.snapshot(), history);
}

#[test]
fn unknown_request_cannot_install_prepared_owner_or_task_receipt() {
    let mut graph = reserved_child();
    let before = graph.snapshot();
    assert_eq!(
        graph.note_prepared_owner(&spawn_id("unknown")),
        Err(OwnershipError::UnknownSpawn)
    );
    assert_eq!(
        graph.note_task_receipt(&spawn_id("unknown"), receipt("actual")),
        Err(OwnershipError::UnknownSpawn)
    );
    assert_eq!(graph.snapshot(), before);
}

#[test]
fn missing_or_foreign_observation_correlation_cannot_acknowledge_local_debt() {
    let mut donor = ready_root("close");
    let actual = donor
        .apply_report(
            &life("root"),
            &close_id("close"),
            &life("root"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    for record in [
        OwnershipEvidence {
            child_lifetime: None,
            ..actual.clone()
        },
        OwnershipEvidence {
            close_operation: None,
            ..actual.clone()
        },
        OwnershipEvidence {
            close_operation: Some(close_id("foreign")),
            ..actual.clone()
        },
    ] {
        let before = donor.snapshot();
        assert_eq!(
            donor.acknowledge_observation(&record, EvidenceFact::Acknowledged),
            Err(OwnershipError::StaleOutcome)
        );
        assert_eq!(donor.snapshot(), before);
    }
    let mut recipient = closing_root("close");
    let before = recipient.snapshot();
    assert_eq!(
        recipient.acknowledge_observation(&actual, EvidenceFact::Acknowledged),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(recipient.snapshot(), before);
}

#[test]
fn completion_receipt_requires_the_ready_recipient_own_prepared_decision() {
    let mut donor = ready_root("close");
    let token = donor
        .prepare_completion(&life("root"), &close_id("close"))
        .unwrap();
    let mut recipient = ready_root("close");
    let before = recipient.snapshot();
    assert_eq!(
        recipient.acknowledge_completion(&token, EvidenceFact::Acknowledged),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(recipient.snapshot(), before);
    let own = recipient
        .prepare_completion(&life("root"), &close_id("close"))
        .unwrap();
    recipient
        .acknowledge_completion(&own, EvidenceFact::Acknowledged)
        .unwrap();
    assert_eq!(
        recipient.lifetime_state(&life("root")),
        Some(LifetimeState::Closed)
    );
}

fn completed_owned_group(independent: bool) -> OwnershipGraph {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "outside");
    admit(&mut graph, "outside", "a", "outside-a");
    admit(&mut graph, "a", "z", "a-z");
    let owner = life(if independent { "a" } else { "outside" });
    graph
        .begin_close(
            &owner,
            close_id("group-close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let targets = if independent {
        vec![life("a"), life("z")]
    } else {
        vec![life("outside"), life("a"), life("z")]
    };
    for target in targets {
        let observation = graph
            .apply_report(
                &owner,
                &close_id("group-close"),
                &target,
                PhysicalFact::Released,
                EvidenceFact::Acknowledged,
            )
            .unwrap();
        graph
            .acknowledge_observation(&observation, EvidenceFact::Acknowledged)
            .unwrap();
    }
    complete(&mut graph, &owner);
    assert!(OwnershipGraph::restore(graph.snapshot())
        .refusal()
        .is_none());
    graph
}

#[test]
fn acknowledged_completion_refuses_a_partly_reopened_owned_group_without_repair() {
    for independent in [false, true] {
        let graph = completed_owned_group(independent);
        let mut history = graph.snapshot();
        history
            .lifetimes
            .iter_mut()
            .find(|row| row.lifetime_id == life("z"))
            .unwrap()
            .state = LifetimeState::Closing;
        let mut restored = OwnershipGraph::restore(history.clone());
        assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));
        assert_eq!(restored.snapshot(), history);
        assert_eq!(
            restored.open_root(session("late"), life("late"), Initiator::Runtime),
            Err(OwnershipError::DispatchRefused)
        );
        assert_eq!(restored.snapshot(), history);
    }
}

#[test]
fn cyclic_restored_ancestry_keeps_its_first_refusal_and_returns_with_completion() {
    const CHILD: &str = "NESSA_CYCLIC_OWNERSHIP_RESTORE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let graph = completed_owned_group(true);
        let mut history = graph.snapshot();
        let edge = history
            .spawns
            .iter_mut()
            .find(|row| row.child_lifetime == life("a"))
            .unwrap();
        edge.binding.parent_lifetime = life("z");
        edge.binding.parent_session = session("z");
        eprintln!("accepted completed control; restoring actual ancestry cycle");
        let mut restored = OwnershipGraph::restore(history.clone());
        assert_eq!(restored.refusal(), Some(&OwnershipError::Cycle));
        assert_eq!(restored.snapshot(), history);
        assert_eq!(
            restored.open_root(session("late"), life("late"), Initiator::Runtime),
            Err(OwnershipError::Cycle)
        );
        assert_eq!(restored.snapshot(), history);
        return;
    }
    crate::subprocess::run(
        "domain::agent_execution::ownership::settlement_relationships::cyclic_restored_ancestry_keeps_its_first_refusal_and_returns_with_completion",
        CHILD,
        std::time::Duration::from_secs(3),
        Some("accepted completed control; restoring actual ancestry cycle"),
    );
}

fn interrupted_cascade_with_retained_proof() -> OwnershipSnapshot {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "outside");
    admit(&mut graph, "outside", "a", "outside-a");
    admit(&mut graph, "a", "z", "a-z");
    graph
        .begin_close(
            &life("outside"),
            close_id("interrupted"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let record = graph
        .apply_report(
            &life("outside"),
            &close_id("interrupted"),
            &life("outside"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    graph
        .acknowledge_observation(&record, EvidenceFact::Acknowledged)
        .unwrap();
    let mut snapshot = graph.snapshot();
    for row in &mut snapshot.lifetimes {
        if row.lifetime_id != life("outside") {
            row.state = LifetimeState::Open;
            row.close_operation = None;
            row.cause = None;
            row.initiator = None;
            row.cascaded_from = None;
        }
    }
    snapshot
}

#[test]
fn refused_proof_keeps_interrupted_cascade_original_rows_and_no_recovery() {
    let history = interrupted_cascade_with_retained_proof();
    let accepted = OwnershipGraph::restore(history.clone());
    assert!(accepted.refusal().is_none());
    assert_eq!(accepted.recovery_records().len(), 2);
    assert_eq!(accepted.snapshot().settlements, history.settlements);
    assert_eq!(
        accepted.lifetime_state(&life("a")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(accepted.close_owner(&life("z")), Some(life("outside")));
    let mut invalid = history;
    invalid.settlements[0].evidence = EvidenceFact::Failed;
    let refused = OwnershipGraph::restore(invalid.clone());
    assert_eq!(refused.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(refused.snapshot(), invalid);
    assert!(refused.recovery_records().is_empty());
}

#[test]
fn completed_ancestor_refuses_open_descendants_without_repairing_retained_history() {
    let graph = completed_owned_group(false);
    let mut history = graph.snapshot();
    for row in &mut history.lifetimes {
        if row.lifetime_id != life("outside") {
            row.state = LifetimeState::Open;
            row.close_operation = None;
            row.cause = None;
            row.initiator = None;
            row.cascaded_from = None;
        }
    }
    history
        .settlements
        .retain(|row| row.target == life("outside"));
    let mut restored = OwnershipGraph::restore(history.clone());
    assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(restored.snapshot(), history);
    assert!(restored.recovery_records().is_empty());
    assert_eq!(
        restored.open_root(session("late"), life("late"), Initiator::Runtime),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(restored.snapshot(), history);
}
