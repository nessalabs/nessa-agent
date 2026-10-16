//! Borrowed inspection retains the owned persistence view, including refused rows.
//! Matrix: live reservation/failure/end and physical/audit differences; accepted
//! reload; structural refusal; duplicate keys and proof refusal. Each preserves
//! the snapshot's retained map order and fields while borrowing the actual rows.
use super::*;
use crate::domain::agent_execution::subagents::values::{
    ApprovalPolicy, HostActor, SpawnOrigin, TaskDigest,
};

fn life(name: &str) -> AgentLifetimeId {
    AgentLifetimeId::new(name).unwrap()
}

fn assert_borrowed_rows(graph: &OwnershipGraph) {
    let snapshot = graph.snapshot();
    assert_eq!(
        graph.spawn_rows().cloned().collect::<Vec<_>>(),
        snapshot.spawns
    );
    assert_eq!(
        graph.settlement_rows().cloned().collect::<Vec<_>>(),
        snapshot.settlements
    );
    assert_eq!(
        graph.close_completion_rows().cloned().collect::<Vec<_>>(),
        snapshot.close_completions
    );
    for (actual, retained) in graph.spawn_rows().zip(graph.spawns.values()) {
        assert!(std::ptr::eq(actual, &retained.row));
    }
    for (actual, retained) in graph.settlement_rows().zip(graph.settlements.values()) {
        assert!(std::ptr::eq(actual, &retained.row));
    }
    for (actual, retained) in graph
        .close_completion_rows()
        .zip(graph.close_completions.values())
    {
        assert!(std::ptr::eq(actual, retained));
    }
}

fn populated() -> OwnershipGraph {
    let mut graph = OwnershipGraph::new();
    for name in ["z-root", "a-root", "m-root", "pending-root", "parent"] {
        let _opened = graph
            .open_root(
                SessionId::new(name).unwrap(),
                life(name),
                Initiator::Runtime,
            )
            .unwrap();
    }
    for (request, progress) in [
        ("z-request", SpawnProgress::Reserved),
        (
            "a-request",
            SpawnProgress::StartupFailed {
                known: KnownMilestone::Reserved,
            },
        ),
        (
            "m-request",
            SpawnProgress::Ended {
                known: KnownMilestone::Reserved,
            },
        ),
    ] {
        let _admitted = graph
            .admit_spawn(SpawnAdmission {
                child_lifetime: life(request),
                child_session: SessionId::new(request).unwrap(),
                binding: SpawnBinding {
                    parent_lifetime: life("parent"),
                    parent_session: SessionId::new("parent").unwrap(),
                    request_id: SpawnRequestId::new(request).unwrap(),
                    task_digest: TaskDigest::new("a".repeat(64)).unwrap(),
                    policy: ApprovalPolicy::new("read-only", "ask", "revision").unwrap(),
                    model: None,
                    origin: SpawnOrigin::Host(
                        HostActor::new("person", "desktop", request).unwrap(),
                    ),
                },
                live_room: true,
            })
            .unwrap();
        if progress != SpawnProgress::Reserved {
            let request = SpawnRequestId::new(request).unwrap();
            let _failed = graph
                .advance_spawn(
                    &request,
                    SpawnProgress::StartupFailed {
                        known: KnownMilestone::Reserved,
                    },
                )
                .unwrap();
            if matches!(progress, SpawnProgress::Ended { .. }) {
                let _ended = graph.advance_spawn(&request, progress).unwrap();
            }
        }
    }
    for (name, acknowledgement) in [
        ("z-root", EvidenceFact::Acknowledged),
        ("a-root", EvidenceFact::Failed),
        ("m-root", EvidenceFact::Pending),
        ("pending-root", EvidenceFact::Pending),
    ] {
        let root = life(name);
        let operation = CloseOperationId::new(name).unwrap();
        let _closing = graph
            .begin_close(
                &root,
                operation.clone(),
                LifetimeCause::OwnerDisposed,
                Initiator::Runtime,
            )
            .unwrap();
        let physical = if name == "pending-root" {
            PhysicalFact::Pending
        } else {
            PhysicalFact::Released
        };
        let record = graph
            .apply_report(
                &root,
                &operation,
                &root,
                physical,
                EvidenceFact::Acknowledged,
            )
            .unwrap();
        if physical == PhysicalFact::Released {
            graph
                .acknowledge_observation(&record, EvidenceFact::Acknowledged)
                .unwrap();
            let token = graph.prepare_completion(&root, &operation).unwrap();
            if acknowledgement != EvidenceFact::Pending {
                graph
                    .acknowledge_completion(&token, acknowledgement)
                    .unwrap();
            }
        }
    }
    graph
}

#[test]
fn borrowed_rows_match_empty_and_live_graphs_and_accepted_reload() {
    assert_borrowed_rows(&OwnershipGraph::new());
    let graph = populated();
    assert_borrowed_rows(&graph);
    let restored = OwnershipGraph::restore(graph.snapshot());
    assert!(restored.refusal().is_none());
    assert_borrowed_rows(&restored);
}

#[test]
fn borrowed_rows_keep_structurally_refused_history_readable() {
    let mut snapshot = populated().snapshot();
    snapshot.spawns[0].binding.parent_lifetime = life("missing-parent");
    let graph = OwnershipGraph::restore(snapshot);
    assert!(graph.refusal().is_some());
    assert_eq!(graph.spawns.len(), 3);
    assert_eq!(graph.settlements.len(), 4);
    assert_eq!(graph.close_completions.len(), 3);
    assert_borrowed_rows(&graph);
}

#[test]
fn borrowed_rows_match_retained_maps_after_duplicate_keys_and_invalid_proof() {
    let valid = populated().snapshot();
    let mut duplicate = valid.clone();
    duplicate.spawns.push(duplicate.spawns[0].clone());
    duplicate.settlements.push(duplicate.settlements[0].clone());
    duplicate
        .close_completions
        .push(duplicate.close_completions[0].clone());
    let mut proof = valid;
    proof.settlements[0].evidence = EvidenceFact::Failed;
    for snapshot in [duplicate, proof] {
        let graph = OwnershipGraph::restore(snapshot);
        assert!(graph.refusal().is_some());
        assert_eq!(graph.spawns.len(), 3);
        assert_eq!(graph.settlements.len(), 4);
        assert_eq!(graph.close_completions.len(), 3);
        assert_borrowed_rows(&graph);
    }
}
