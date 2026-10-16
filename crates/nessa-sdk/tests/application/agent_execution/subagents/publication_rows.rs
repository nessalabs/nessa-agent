//! Publication eligibility across ancestor chains and acknowledgement orderings.
use super::*;
use crate::domain::agent_execution::{
    sessions::SessionId,
    subagents::{
        ApprovalPolicy, HostActor, Initiator, SpawnAdmission, SpawnBinding, SpawnOrigin, TaskDigest,
    },
};

fn chain() -> (OwnershipGraph, [AgentLifetimeId; 3], [SpawnRequestId; 2]) {
    let lifetimes = ["root", "child", "grandchild"].map(|id| AgentLifetimeId::new(id).unwrap());
    // Graph rows are ordered by request identity, so the descendant is visited
    // before its parent and needs a later fixed-point pass.
    let requests = ["z-child", "a-grandchild"].map(|id| SpawnRequestId::new(id).unwrap());
    let mut graph = OwnershipGraph::new();
    let _opened = graph
        .open_root(
            SessionId::new("root").unwrap(),
            lifetimes[0].clone(),
            Initiator::Runtime,
        )
        .unwrap();
    for index in 0..2 {
        let _admitted = graph
            .admit_spawn(SpawnAdmission {
                child_lifetime: lifetimes[index + 1].clone(),
                child_session: SessionId::new(lifetimes[index + 1].as_str()).unwrap(),
                live_room: true,
                binding: SpawnBinding {
                    parent_lifetime: lifetimes[index].clone(),
                    parent_session: SessionId::new(lifetimes[index].as_str()).unwrap(),
                    request_id: requests[index].clone(),
                    task_digest: TaskDigest::new("0".repeat(64)).unwrap(),
                    policy: ApprovalPolicy::new("read-only", "ask", "revision").unwrap(),
                    model: None,
                    origin: SpawnOrigin::Host(
                        HostActor::new("person", "desktop", "chain").unwrap(),
                    ),
                },
            })
            .unwrap();
    }
    (graph, lifetimes, requests)
}

fn assert_retained(
    publication: &OwnershipPublication,
    graph: &OwnershipGraph,
    lifetimes: &[AgentLifetimeId; 3],
    expected: &[&str],
    requests: &[&str],
) {
    for lifetime in lifetimes {
        assert_eq!(
            publication.lifetime_eligible(graph, lifetime),
            expected.contains(&lifetime.as_str()),
            "eligibility for {lifetime:?}",
        );
    }
    let projection = publication.project(graph);
    assert_eq!(
        projection
            .lifetimes
            .iter()
            .map(|row| row.lifetime_id.as_str())
            .collect::<Vec<_>>(),
        expected,
    );
    assert_eq!(
        projection
            .spawns
            .iter()
            .map(|row| row.binding.request_id.as_str())
            .collect::<Vec<_>>(),
        requests,
    );
    assert!(OwnershipGraph::restore(projection).refusal().is_none());
}

#[test]
fn reverse_request_order_retains_acknowledged_grandchild_after_multiple_passes() {
    let (graph, lifetimes, _) = chain();
    let snapshot = graph.snapshot();
    let publication = OwnershipPublication::restored(&snapshot);
    assert_retained(
        &publication,
        &graph,
        &lifetimes,
        &["child", "grandchild", "root"],
        &["a-grandchild", "z-child"],
    );
    assert_eq!(publication.project(&graph), snapshot);
}

#[test]
fn ancestor_publication_and_current_generation_control_chain_visibility() {
    let (mut graph, lifetimes, requests) = chain();
    let mut withheld_root = OwnershipPublication::default();
    for request in &requests {
        let token = withheld_root.begin(
            PublicationTarget::Spawn(request.clone()),
            PublishedState::Spawn(SpawnProgress::Reserved),
        );
        assert!(withheld_root.acknowledge(&token));
    }
    assert_retained(&withheld_root, &graph, &lifetimes, &[], &[]);

    let mut publication = OwnershipPublication::default();
    let root = publication.begin(
        PublicationTarget::Root(lifetimes[0].clone()),
        PublishedState::Root,
    );
    assert!(publication.acknowledge(&root));
    let grandchild = publication.begin(
        PublicationTarget::Spawn(requests[1].clone()),
        PublishedState::Spawn(SpawnProgress::Reserved),
    );
    assert!(publication.acknowledge(&grandchild));
    assert_retained(&publication, &graph, &lifetimes, &["root"], &[]);

    let old_child = publication.begin(
        PublicationTarget::Spawn(requests[0].clone()),
        PublishedState::Spawn(SpawnProgress::Reserved),
    );
    let _prepared = graph
        .advance_spawn(&requests[0], SpawnProgress::Prepared)
        .unwrap();
    let current_child = publication.begin(
        PublicationTarget::Spawn(requests[0].clone()),
        PublishedState::Spawn(SpawnProgress::Prepared),
    );
    assert!(!publication.acknowledge(&old_child));
    assert_retained(&publication, &graph, &lifetimes, &["root"], &[]);
    assert!(publication.acknowledge(&current_child));
    assert_retained(
        &publication,
        &graph,
        &lifetimes,
        &["child", "grandchild", "root"],
        &["a-grandchild", "z-child"],
    );
    assert_eq!(publication.project(&graph), graph.snapshot());

    // A retained root identity can precede its presence in this live graph.
    let unknown = AgentLifetimeId::new("retained-unknown-root").unwrap();
    publication.retain_root(&unknown);
    assert!(publication.lifetime_eligible(&graph, &unknown));
    assert_eq!(publication.project(&graph), graph.snapshot());
}
