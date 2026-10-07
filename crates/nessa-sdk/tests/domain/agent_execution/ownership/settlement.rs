//! Bounded exact close debt, physical/provider correlation and Completion authority.
use super::*;

fn closing() -> OwnershipGraph {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "root");
    graph.begin_close(&life("root"), close_id("close"), LifetimeCause::HostClose, Initiator::Host(actor("close"))).unwrap();
    graph
}
fn observed(graph: &mut OwnershipGraph, physical: PhysicalFact, evidence: EvidenceFact) -> OwnershipEvidence {
    graph.apply_report(&life("root"), &close_id("close"), &life("root"), physical, evidence).unwrap()
}

#[test]
fn three_first_observations_survive_rejected_audits_and_release_advancement() {
    let mut graph = closing();
    let mut exact = Vec::new();
    for physical in [PhysicalFact::Pending, PhysicalFact::Failed, PhysicalFact::Released] {
        let record = observed(&mut graph, physical, EvidenceFact::Acknowledged);
        graph.acknowledge_observation(&record, EvidenceFact::Failed).unwrap();
        exact.push(record.clone());
        assert_eq!(observed(&mut graph, physical, EvidenceFact::Failed), record);
    }
    assert_eq!(graph.pending_close_evidence(&life("root")), exact);
    assert_eq!(graph.physical(&life("root"), &life("root")), Some(PhysicalFact::Released));
    assert!(graph.prepare_completion(&life("root"), &close_id("close")).is_err());
    let snapshot = graph.snapshot();
    let SettlementProof::Resource(slots) = &snapshot.settlements[0].proof else { panic!("resource authority"); };
    assert_eq!(slots.iter().flatten().count(), 3);
    let mut restored = OwnershipGraph::restore(snapshot.clone());
    assert!(restored.refusal().is_none());
    assert_eq!(restored.pending_close_evidence(&life("root")), exact);
    for record in &exact { restored.acknowledge_observation(record, EvidenceFact::Acknowledged).unwrap(); }
    assert_eq!(restored.lifetime_state(&life("root")), Some(LifetimeState::Closing));
    let completion = restored.prepare_completion(&life("root"), &close_id("close")).unwrap();
    assert_eq!(completion.evidence().close_detail, Some(CloseEvidenceDetail::Completion));
    assert_eq!(restored.acknowledge_observation(completion.evidence(), EvidenceFact::Acknowledged), Err(OwnershipError::StaleOutcome));
    restored.acknowledge_completion(&completion, EvidenceFact::Failed).unwrap();
    assert_eq!(restored.lifetime_state(&life("root")), Some(LifetimeState::Closing));
    restored.acknowledge_completion(&completion, EvidenceFact::Acknowledged).unwrap();
    restored.acknowledge_completion(&completion, EvidenceFact::Failed).unwrap();
    assert_eq!(restored.lifetime_state(&life("root")), Some(LifetimeState::Closed));
    assert!(OwnershipGraph::restore(restored.snapshot()).refusal().is_none());
    let before = restored.snapshot();
    assert!(restored.apply_report(&life("root"), &close_id("close"), &life("root"), PhysicalFact::Failed, EvidenceFact::Acknowledged).is_err());
    assert_eq!(restored.snapshot(), before);
}

#[test]
fn failed_provider_witness_cannot_acknowledge_release_and_legitimate_improvement_keeps_record() {
    let mut graph = closing();
    let failure = observed(&mut graph, PhysicalFact::Failed, EvidenceFact::Acknowledged);
    let released = observed(&mut graph, PhysicalFact::Released, EvidenceFact::Failed);
    graph.acknowledge_observation(&failure, EvidenceFact::Acknowledged).unwrap();
    graph.acknowledge_observation(&released, EvidenceFact::Acknowledged).unwrap();
    assert!(graph.prepare_completion(&life("root"), &close_id("close")).is_err());
    let again = observed(&mut graph, PhysicalFact::Released, EvidenceFact::Acknowledged);
    assert_eq!(again, released);
    assert_eq!(observed(&mut graph, PhysicalFact::Released, EvidenceFact::Failed), released);
    assert!(graph.pending_close_evidence(&life("root")).is_empty());
    complete(&mut graph, &life("root"));
    assert!(OwnershipGraph::restore(graph.snapshot()).refusal().is_none());
}

#[test]
fn independent_child_completion_keeps_its_first_owner_and_blocks_parent() {
    let mut graph = OwnershipGraph::new(); root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "request");
    graph.begin_close(&life("child"), close_id("child-close"), LifetimeCause::TerminalFailure, Initiator::Runtime).unwrap();
    graph.note_absence(&life("child"), &close_id("child-close"), &life("child"), AbsenceProof::PreparationRejectedWithoutOwner(spawn_id("request"))).unwrap();
    graph.begin_close(&life("parent"), close_id("parent-close"), LifetimeCause::Deletion, Initiator::Host(actor("delete"))).unwrap();
    graph.note_unbound_root(&life("parent"), &close_id("parent-close")).unwrap();
    for record in graph.pending_close_evidence(&life("parent")) { graph.acknowledge_observation(&record, EvidenceFact::Acknowledged).unwrap(); }
    assert_eq!(graph.close_owner(&life("child")), Some(life("child")));
    assert_eq!(graph.independent_closes(&life("parent")), vec![life("child")]);
    assert!(graph.physical_targets(&life("parent")).is_empty());
    assert!(graph.prepare_completion(&life("parent"), &close_id("parent-close")).is_err());
    assert!(graph.apply_report(&life("parent"), &close_id("parent-close"), &life("child"), PhysicalFact::Released, EvidenceFact::Acknowledged).is_err());
    complete(&mut graph, &life("child")); complete(&mut graph, &life("parent"));
    assert_eq!(graph.close_cause(&life("child")), Some(&LifetimeCause::TerminalFailure));
    assert_eq!(graph.close_initiator(&life("child")), Some(&Initiator::Runtime));
    assert_eq!(graph.snapshot().close_completions.len(), 2);
    assert!(OwnershipGraph::restore(graph.snapshot()).refusal().is_none());
}

#[test]
fn absence_proof_rejects_foreign_request_and_restored_contradictions_without_repair() {
    let mut graph = closing();
    assert!(graph.note_absence(&life("root"), &close_id("close"), &life("root"), AbsenceProof::AdmissionFailedBeforeFactory(spawn_id("foreign"))).is_err());
    let exact = graph.note_unbound_root(&life("root"), &close_id("close")).unwrap();
    graph.acknowledge_unbound_root(exact, EvidenceFact::Acknowledged).unwrap();
    complete(&mut graph, &life("root"));
    let valid = graph.snapshot();
    for mutation in 0..6 {
        let mut history = valid.clone();
        match mutation {
            0 => history.settlements[0].target = life("foreign"),
            1 => history.settlements[0].evidence = EvidenceFact::Failed,
            2 => history.settlements.push(history.settlements[0].clone()),
            3 => history.close_completions.clear(),
            4 => history.lifetimes[0].cause = Some(LifetimeCause::Deletion),
            _ => history.lifetimes[0].state = LifetimeState::Closing,
        }
        let restored = OwnershipGraph::restore(history.clone());
        assert!(restored.refusal().is_some(), "contradiction {mutation}");
        // Restore retains the contradictory authority instead of repairing it.
        assert_eq!(restored.snapshot().lifetimes, history.lifetimes);
    }
    let mut child = OwnershipGraph::new(); root(&mut child, "parent"); admit(&mut child, "parent", "child", "request");
    child.begin_close(&life("child"), close_id("child-close"), LifetimeCause::TerminalFailure, Initiator::Runtime).unwrap();
    for proof in [AbsenceProof::PreparationRejectedWithoutOwner(spawn_id("request")), AbsenceProof::AdmissionFailedBeforeFactory(spawn_id("request"))] {
        let mut possible = child.clone();
        possible.note_absence(&life("child"), &close_id("child-close"), &life("child"), proof).unwrap();
        assert!(possible.has_absence(&life("child")));
        assert!(OwnershipGraph::restore(possible.snapshot()).refusal().is_none());
    }
    assert!(child.note_absence(&life("child"), &close_id("child-close"), &life("child"), AbsenceProof::PreparationRejectedWithoutOwner(spawn_id("other"))).is_err());
    assert!(child.note_unbound_root(&life("child"), &close_id("child-close")).is_err());
}
