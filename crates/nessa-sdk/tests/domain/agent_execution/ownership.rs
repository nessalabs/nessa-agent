//! Ownership graph rows from ADR 329. Each test names the table row it locks.
//! Settlement children own bounded evidence and public retained-history relationships.
#![allow(unused_must_use)]
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::agent_execution::subagents::*;
use nessa_sdk::domain::agent_execution::{executions::ExecutionId, tools::ToolCallId};

fn session(name: &str) -> SessionId {
    SessionId::new(name).unwrap()
}
fn life(name: &str) -> AgentLifetimeId {
    AgentLifetimeId::new(name).unwrap()
}
fn spawn_id(name: &str) -> SpawnRequestId {
    SpawnRequestId::new(name).unwrap()
}
fn close_id(name: &str) -> CloseOperationId {
    CloseOperationId::new(name).unwrap()
}
fn report(name: &str) -> ReportId {
    ReportId::new(name).unwrap()
}
fn receipt(name: &str) -> TaskReceiptId {
    TaskReceiptId::new(name).unwrap()
}
fn digest(seed: &str) -> TaskDigest {
    let mut hex = String::new();
    for byte in seed.bytes() {
        hex.push_str(&format!("{byte:02x}"));
    }
    while hex.len() < 64 {
        hex.push('0');
    }
    TaskDigest::new(&hex[..64]).unwrap()
}
fn policy(revision: &str) -> ApprovalPolicy {
    ApprovalPolicy::new("read-only", "ask", revision).unwrap()
}
fn actor(request: &str) -> HostActor {
    HostActor::new("person", "desktop", request).unwrap()
}
fn binding(parent: &str, request: &str, task: &str) -> SpawnBinding {
    SpawnBinding {
        parent_lifetime: life(parent),
        parent_session: session(parent),
        request_id: spawn_id(request),
        task_digest: digest(task),
        policy: policy("rev-1"),
        model: None,
        origin: SpawnOrigin::Host(actor(request)),
    }
}
fn root(graph: &mut OwnershipGraph, name: &str) {
    graph
        .open_root(session(name), life(name), Initiator::Host(actor("open")))
        .unwrap();
    assert_eq!(
        graph.session_id(&life(name)).map(SessionId::as_str),
        Some(name)
    );
}
fn admit(
    graph: &mut OwnershipGraph,
    parent: &str,
    child: &str,
    request: &str,
) -> OwnershipEvidence {
    graph
        .admit_spawn(SpawnAdmission {
            child_lifetime: life(child),
            child_session: session(child),
            binding: binding(parent, request, request),
            live_room: true,
        })
        .unwrap()
}

fn complete(graph: &mut OwnershipGraph, root: &AgentLifetimeId) {
    for record in graph.pending_close_evidence(root) {
        graph
            .acknowledge_observation(&record, EvidenceFact::Acknowledged)
            .unwrap();
    }
    let operation = graph.close_operation(root).cloned().unwrap();
    let completion = graph.prepare_completion(root, &operation).unwrap();
    graph
        .acknowledge_completion(&completion, EvidenceFact::Acknowledged)
        .unwrap();
}

#[path = "ownership/settlement.rs"]
mod settlement;
mod settlement_relationships;

#[test]
fn identity_and_policy_values_reject_blank_and_oversized_input() {
    assert!(matches!(
        AgentLifetimeId::new(""),
        Err(OwnershipError::InvalidIdentity(_))
    ));
    assert!(matches!(
        AgentLifetimeId::new("has space"),
        Err(OwnershipError::InvalidIdentity(_))
    ));
    assert!(matches!(
        SpawnRequestId::new("x".repeat(129)),
        Err(OwnershipError::InvalidIdentity(_))
    ));
    assert!(matches!(
        CloseOperationId::new(""),
        Err(OwnershipError::InvalidIdentity(_))
    ));
    assert!(matches!(
        ReportId::new(""),
        Err(OwnershipError::InvalidIdentity(_))
    ));
    assert!(matches!(
        TaskReceiptId::new(""),
        Err(OwnershipError::InvalidIdentity(_))
    ));
    assert!(CloseOperationId::new("ok-1").is_ok());
    assert!(matches!(
        TaskDigest::new("abc"),
        Err(OwnershipError::InvalidTaskDigest)
    ));
    assert!(matches!(
        TaskDigest::new("A".repeat(64)),
        Err(OwnershipError::InvalidTaskDigest)
    ));
    assert!(matches!(
        TaskDigest::new("g".repeat(64)),
        Err(OwnershipError::InvalidTaskDigest)
    ));
    assert!(AgentLifetimeId::new("a_b").is_ok());
    assert!(digest("b").as_str().len() == 64);
    assert_eq!(life("parent").as_str(), "parent");
    assert_eq!(spawn_id("req-1").as_str(), "req-1");
    assert_eq!(close_id("close-1").as_str(), "close-1");
    assert_eq!(report("rep-1").as_str(), "rep-1");
    assert_eq!(receipt("receipt-1").as_str(), "receipt-1");
    let admitted = SpawnProgress::TaskAdmitted {
        receipt: receipt("receipt-1"),
    };
    assert_eq!(
        admitted.known(),
        KnownMilestone::TaskAdmitted {
            receipt: receipt("receipt-1"),
        }
    );
    assert_eq!(
        SpawnProgress::Unconfirmed {
            known: KnownMilestone::Reserved,
        }
        .known(),
        KnownMilestone::Reserved
    );
    assert_eq!(
        SpawnProgress::Draining {
            known: KnownMilestone::Prepared,
        }
        .known(),
        KnownMilestone::Prepared
    );
    assert_eq!(
        SpawnProgress::StartupFailed {
            known: KnownMilestone::Attached,
        }
        .known(),
        KnownMilestone::Attached
    );
    assert_eq!(
        SpawnProgress::Ended {
            known: KnownMilestone::Reserved,
        }
        .known(),
        KnownMilestone::Reserved
    );
    assert!(matches!(
        ApprovalPolicy::new("  ", "ask", "rev"),
        Err(OwnershipError::EmptyValue(_))
    ));
    assert!(matches!(
        ApprovalPolicy::new("ok", " ", "rev"),
        Err(OwnershipError::EmptyValue(_))
    ));
    assert!(matches!(
        ApprovalPolicy::new("ok", "ask", " "),
        Err(OwnershipError::EmptyValue(_))
    ));
    assert!(matches!(
        ApprovalPolicy::new("m".repeat(MAX_LABEL_BYTES + 1), "ask", "rev"),
        Err(OwnershipError::ValueTooLong { .. })
    ));
    let selected = select_inherited_policy(PolicyRead::Committed(policy("rev-1")), true).unwrap();
    assert_eq!(selected.mode(), "read-only");
    assert_eq!(selected.offer(), "ask");
    assert_eq!(selected.revision(), "rev-1");
    assert_eq!(
        select_inherited_policy(PolicyRead::Pending, true),
        Err(OwnershipError::PolicyPending)
    );
    assert_eq!(
        select_inherited_policy(PolicyRead::Uncertain, true),
        Err(OwnershipError::PolicyUncertain)
    );
    assert_eq!(
        select_inherited_policy(PolicyRead::ParentUnavailable, true),
        Err(OwnershipError::ParentClosing)
    );
    assert_eq!(
        select_inherited_policy(PolicyRead::Committed(policy("rev-1")), false),
        Err(OwnershipError::UnsupportedPolicy)
    );
    assert!(matches!(
        ModelChoice::new("", "m"),
        Err(OwnershipError::EmptyValue(_))
    ));
    assert!(matches!(
        ModelChoice::new("provider", " "),
        Err(OwnershipError::EmptyValue(_))
    ));
    let choice = ModelChoice::new("codex", "gpt").unwrap();
    assert_eq!(choice.provider(), "codex");
    assert_eq!(choice.model(), "gpt");
    assert!(matches!(
        HostActor::new("p", " ", "r"),
        Err(OwnershipError::EmptyValue(_))
    ));
    assert!(matches!(
        HostActor::new("", "surface", "req"),
        Err(OwnershipError::EmptyValue(_))
    ));
    assert!(matches!(
        HostActor::new("person", "surface", " "),
        Err(OwnershipError::EmptyValue(_))
    ));
    let host = actor("req");
    assert_eq!(host.principal_id(), "person");
    assert_eq!(host.surface_id(), "desktop");
    assert_eq!(host.request_id(), "req");
    let execution = SpawnOrigin::Execution {
        execution: ExecutionId::new("exec-1").unwrap(),
        tool: Some(ToolCallId::new("tool-1").unwrap()),
    };
    assert!(matches!(execution, SpawnOrigin::Execution { .. }));
}

#[test]
fn every_ownership_error_formats_as_text_and_has_no_source() {
    let errors = [
        OwnershipError::EmptyValue("field"),
        OwnershipError::ValueTooLong {
            field: "field",
            max_bytes: 1,
        },
        OwnershipError::InvalidIdentity("field"),
        OwnershipError::InvalidTaskDigest,
        OwnershipError::ParentMissing,
        OwnershipError::ParentClosing,
        OwnershipError::ParentClosed,
        OwnershipError::ChildUnavailable,
        OwnershipError::RequestConflict,
        OwnershipError::UnsupportedPolicy,
        OwnershipError::PolicyPending,
        OwnershipError::PolicyUncertain,
        OwnershipError::NoRoom,
        OwnershipError::DirectChildrenExceeded,
        OwnershipError::DepthExceeded,
        OwnershipError::RetainedRequestsExceeded,
        OwnershipError::PageLimit,
        OwnershipError::Cycle,
        OwnershipError::ForeignParent,
        OwnershipError::Contradictory,
        OwnershipError::DispatchRefused,
        OwnershipError::LifetimeStillOpen,
        OwnershipError::UnknownSpawn,
        OwnershipError::IllegalSpawnProgress,
        OwnershipError::StaleOutcome,
        OwnershipError::SelfParent,
        OwnershipError::ReportConflict,
        OwnershipError::UnknownChild,
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[test]
fn s1_open_parent_reserves_one_child_before_any_later_progress() {
    let mut graph = OwnershipGraph::default();
    root(&mut graph, "parent");
    let reserved = admit(&mut graph, "parent", "child", "req-1");
    assert_eq!(reserved.before, OwnershipMeaning::Absent);
    assert_eq!(reserved.after, OwnershipMeaning::Reserved);
    assert_eq!(
        graph.child_lifetime(&spawn_id("req-1")),
        Some(&life("child"))
    );
    assert_eq!(
        graph.spawn_progress(&spawn_id("req-1")),
        Some(&SpawnProgress::Reserved)
    );
    let prepared = graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared)
        .unwrap();
    assert_eq!(prepared.after, OwnershipMeaning::Prepared);
    assert_eq!(
        graph.dispatch_after_prepare(&spawn_id("req-1")).unwrap(),
        Dispatch::Allowed
    );
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Attached)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::TaskAdmitted {
                receipt: receipt("task-1"),
            },
        )
        .unwrap();
    assert!(matches!(
        graph.spawn_progress(&spawn_id("req-1")),
        Some(SpawnProgress::TaskAdmitted { .. })
    ));
}

#[test]
fn s2_identical_retry_returns_the_original_child() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    let again = admit(&mut graph, "parent", "other", "req-1");
    assert_eq!(again.child_lifetime, Some(life("child")));
    assert_eq!(graph.lifetime_state(&life("other")), None);
}

#[test]
fn s3_changed_binding_conflicts_and_keeps_the_original() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    let mut changed = binding("parent", "req-1", "other-task");
    changed.task_digest = digest("zzz");
    let error = graph.admit_spawn(SpawnAdmission {
        child_lifetime: life("child-2"),
        child_session: session("child-2"),
        binding: changed,
        live_room: true,
    });
    assert_eq!(error, Err(OwnershipError::RequestConflict));
    assert_eq!(
        graph.child_lifetime(&spawn_id("req-1")),
        Some(&life("child"))
    );
}

#[test]
fn s4_close_before_reservation_refuses_the_spawn() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close")),
        )
        .unwrap();
    let error = graph.admit_spawn(SpawnAdmission {
        child_lifetime: life("child"),
        child_session: session("child"),
        binding: binding("parent", "req-1", "task"),
        live_room: true,
    });
    assert_eq!(error, Err(OwnershipError::ParentClosing));
    assert!(graph.spawn_progress(&spawn_id("req-1")).is_none());
}

#[test]
fn s5_reservation_joins_the_drain_and_cannot_dispatch() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close")),
        )
        .unwrap();
    assert_eq!(
        graph.dispatch_after_prepare(&spawn_id("req-1")).unwrap(),
        Dispatch::Drain
    );
    assert!(matches!(
        graph.spawn_progress(&spawn_id("req-1")),
        Some(SpawnProgress::Draining { .. })
    ));
}

#[test]
fn dispatch_after_prepare_refuses_a_child_that_is_not_open() {
    let mut closing = OwnershipGraph::new();
    root(&mut closing, "parent");
    admit(&mut closing, "parent", "child", "req-1");
    closing
        .begin_close(
            &life("child"),
            close_id("close-child"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close-child")),
        )
        .unwrap();
    assert_eq!(
        closing.lifetime_state(&life("parent")),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        closing.dispatch_after_prepare(&spawn_id("req-1")).unwrap(),
        Dispatch::ChildUnavailable
    );
    assert!(matches!(
        closing.spawn_progress(&spawn_id("req-1")),
        Some(SpawnProgress::Draining { .. })
    ));

    let mut admitted = OwnershipGraph::new();
    root(&mut admitted, "parent");
    admit(&mut admitted, "parent", "child", "req-1");
    admitted
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared)
        .unwrap();
    admitted
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Attached)
        .unwrap();
    admitted
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::TaskAdmitted {
                receipt: receipt("task-1"),
            },
        )
        .unwrap();
    admitted
        .begin_close(
            &life("child"),
            close_id("close-admitted"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close-admitted")),
        )
        .unwrap();
    assert_eq!(
        admitted.dispatch_after_prepare(&spawn_id("req-1")).unwrap(),
        Dispatch::ChildUnavailable
    );
    assert!(matches!(
        admitted.spawn_progress(&spawn_id("req-1")),
        Some(SpawnProgress::TaskAdmitted { .. })
    ));

    let mut closed = OwnershipGraph::new();
    root(&mut closed, "parent");
    admit(&mut closed, "parent", "child", "req-1");
    closed
        .begin_close(
            &life("child"),
            close_id("close-closed"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close-closed")),
        )
        .unwrap();
    closed
        .apply_report(
            &life("child"),
            &close_id("close-closed"),
            &life("child"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut closed, &life("child"));
    assert_eq!(
        closed.lifetime_state(&life("child")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        closed.dispatch_after_prepare(&spawn_id("req-1")).unwrap(),
        Dispatch::ChildUnavailable
    );
}

#[test]
fn s6_startup_failure_retains_the_reservation() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    let failed = graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::StartupFailed {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    assert_eq!(failed.after, OwnershipMeaning::StartupFailed);
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Ended {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    assert!(matches!(
        graph.spawn_progress(&spawn_id("req-1")),
        Some(SpawnProgress::Ended { .. })
    ));
}

#[test]
fn s11_exact_direct_child_depth_and_retained_request_bounds() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    for index in 0..MAX_DIRECT_CHILDREN {
        admit(
            &mut graph,
            "parent",
            &format!("child-{index}"),
            &format!("req-{index}"),
        );
    }
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("overflow"),
            child_session: session("overflow"),
            binding: binding("parent", "req-overflow", "task"),
            live_room: true,
        }),
        Err(OwnershipError::DirectChildrenExceeded)
    );
    let mut deep = OwnershipGraph::new();
    root(&mut deep, "root");
    let mut parent = "root".to_string();
    for depth in 1..=MAX_DEPTH {
        let child = format!("d{depth}");
        admit(&mut deep, &parent, &child, &format!("deep-{depth}"));
        parent = child;
    }
    assert_eq!(
        deep.admit_spawn(SpawnAdmission {
            child_lifetime: life("too-deep"),
            child_session: session("too-deep"),
            binding: binding(&parent, "too-deep", "task"),
            live_room: true,
        }),
        Err(OwnershipError::DepthExceeded)
    );
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("no-room"),
            child_session: session("no-room"),
            binding: binding("parent", "no-room", "task"),
            live_room: false,
        }),
        Err(OwnershipError::NoRoom)
    );
}

#[test]
fn retained_request_limit_is_the_published_constant() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    // Direct-child limit would trip first, so close each child before the next reserve.
    for index in 0..MAX_RETAINED_REQUESTS {
        let child = format!("c{index}");
        admit(&mut graph, "parent", &child, &format!("r{index}"));
        let closed = graph
            .begin_close(
                &life(&child),
                close_id(&format!("k{index}")),
                LifetimeCause::HostClose,
                Initiator::Host(actor("close")),
            )
            .unwrap();
        assert!(!closed.joined);
        graph
            .apply_report(
                &life(&child),
                &close_id(&format!("k{index}")),
                &life(&child),
                PhysicalFact::Released,
                EvidenceFact::Acknowledged,
            )
            .unwrap();
        complete(&mut graph, &life(&child));
        assert_eq!(
            graph.lifetime_state(&life(&child)),
            Some(LifetimeState::Closed)
        );
    }
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("past-requests"),
            child_session: session("past-requests"),
            binding: binding("parent", "past-requests", "task"),
            live_room: true,
        }),
        Err(OwnershipError::RetainedRequestsExceeded)
    );
}

#[test]
fn s12_foreign_missing_and_self_parent_refuse_before_a_child_exists() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("child"),
            child_session: session("child"),
            binding: SpawnBinding {
                parent_session: session("other"),
                ..binding("parent", "req-1", "task")
            },
            live_room: true,
        }),
        Err(OwnershipError::ForeignParent)
    );
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("child"),
            child_session: session("child"),
            binding: SpawnBinding {
                parent_lifetime: life("missing"),
                parent_session: session("missing"),
                ..binding("parent", "req-2", "task")
            },
            live_room: true,
        }),
        Err(OwnershipError::ParentMissing)
    );
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("parent"),
            child_session: session("child"),
            binding: binding("parent", "req-3", "task"),
            live_room: true,
        }),
        Err(OwnershipError::SelfParent)
    );
    assert!(graph.spawn_progress(&spawn_id("req-1")).is_none());
}

#[test]
fn closed_parent_and_second_root_are_refused() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    assert_eq!(
        graph.open_root(session("parent"), life("other"), Initiator::Runtime),
        Err(OwnershipError::LifetimeStillOpen)
    );
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::Deletion,
            Initiator::Host(actor("delete")),
        )
        .unwrap();
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("parent"));
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("child"),
            child_session: session("child"),
            binding: binding("parent", "req-1", "task"),
            live_room: true,
        }),
        Err(OwnershipError::ParentClosed)
    );
    graph
        .open_root(
            session("parent"),
            life("parent-2"),
            Initiator::Host(actor("reopen")),
        )
        .unwrap();
    assert_eq!(
        graph.lifetime_state(&life("parent-2")),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        graph.open_root(session("parent"), life("parent"), Initiator::Runtime),
        Err(OwnershipError::LifetimeStillOpen)
    );
    graph
        .begin_close(
            &life("parent-2"),
            close_id("close-2"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close-2")),
        )
        .unwrap();
    graph
        .apply_report(
            &life("parent-2"),
            &close_id("close-2"),
            &life("parent-2"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("parent-2"));
    assert_eq!(
        graph.open_root(session("parent"), life("parent"), Initiator::Runtime),
        Err(OwnershipError::Contradictory)
    );
}

#[test]
fn c1_parent_close_seals_grandchildren_and_keeps_the_root_cause() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-child");
    admit(&mut graph, "child", "grand", "req-grand");
    let initiator = Initiator::Host(actor("close"));
    let admission = graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            initiator.clone(),
        )
        .unwrap();
    assert!(!admission.joined);
    assert_eq!(admission.evidence.cause, Some(LifetimeCause::HostClose));
    assert_eq!(admission.evidence.initiator, initiator);
    assert_eq!(
        graph.lifetime_state(&life("grand")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        graph.close_cause(&life("grand")),
        Some(&LifetimeCause::HostClose)
    );
    assert_eq!(graph.close_initiator(&life("grand")), Some(&initiator));
    assert_eq!(graph.cascaded_from(&life("grand")), Some(&life("child")));
    assert_eq!(graph.cascaded_from(&life("child")), Some(&life("parent")));
    assert!(admission.targets.contains(&life("grand")));
    let again = graph
        .begin_close(
            &life("parent"),
            close_id("close-2"),
            LifetimeCause::TerminalFailure,
            Initiator::Runtime,
        )
        .unwrap();
    assert!(again.joined);
    assert_eq!(
        graph.close_cause(&life("parent")),
        Some(&LifetimeCause::HostClose)
    );
    assert_eq!(again.evidence.close_operation, Some(close_id("close-1")));
}

#[test]
fn c8_direct_child_close_leaves_the_parent_open() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-child");
    admit(&mut graph, "parent", "sibling", "req-sibling");
    graph
        .begin_close(
            &life("child"),
            close_id("child-close"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("child")),
        )
        .unwrap();
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        graph.lifetime_state(&life("sibling")),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        graph.lifetime_state(&life("child")),
        Some(LifetimeState::Closing)
    );
    graph
        .begin_close(
            &life("parent"),
            close_id("parent-close"),
            LifetimeCause::ModeRecovery,
            Initiator::Runtime,
        )
        .unwrap();
    assert_eq!(
        graph.close_cause(&life("child")),
        Some(&LifetimeCause::HostClose)
    );
    assert_eq!(
        graph.close_cause(&life("sibling")),
        Some(&LifetimeCause::ModeRecovery)
    );
}

#[test]
fn cleanup_settles_only_correlated_released_and_acknowledged_targets() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-child");
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::GatewayRetirement,
            Initiator::Runtime,
        )
        .unwrap();
    assert_eq!(
        graph.apply_report(
            &life("parent"),
            &close_id("other"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        ),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(
        graph.apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("outsider"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        ),
        Err(OwnershipError::StaleOutcome)
    );
    let failed = graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("child"),
            PhysicalFact::Failed,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    assert_eq!(failed.after, OwnershipMeaning::Closing);
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Failed,
        )
        .unwrap();
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closing)
    );
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("child"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("parent"));
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        graph.lifetime_state(&life("child")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        graph.physical(&life("parent"), &life("child")),
        Some(PhysicalFact::Released)
    );
    let joined = graph
        .begin_close(
            &life("parent"),
            close_id("again"),
            LifetimeCause::OwnerDisposed,
            Initiator::Runtime,
        )
        .unwrap();
    assert!(joined.joined);
    assert!(joined.targets.is_empty());
    assert_eq!(joined.evidence.after, OwnershipMeaning::Closed);
}

#[test]
fn c12_report_follows_parent_lifetime_and_keeps_an_admitted_report() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-child");
    let submitted = graph
        .admit_report(report("rep-1"), &life("child"), &life("parent"))
        .unwrap();
    assert_eq!(submitted.after, OwnershipMeaning::Submitted);
    assert_eq!(
        graph
            .admit_report(report("rep-1"), &life("child"), &life("parent"))
            .unwrap()
            .after,
        OwnershipMeaning::Submitted
    );
    assert_eq!(
        graph.admit_report(report("rep-1"), &life("other"), &life("parent")),
        Err(OwnershipError::ReportConflict)
    );
    assert_eq!(
        graph.admit_report(report("rep-2"), &life("missing"), &life("parent")),
        Err(OwnershipError::UnknownChild)
    );
    let uncertain = graph.resolve_report(&report("rep-1"), None).unwrap();
    assert_eq!(uncertain.after, OwnershipMeaning::Unconfirmed);
    let still = graph.resolve_report(&report("rep-1"), None).unwrap();
    assert_eq!(still.after, OwnershipMeaning::Unconfirmed);
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close")),
        )
        .unwrap();
    let suppressed = graph.resolve_report(&report("rep-1"), None).unwrap();
    assert_eq!(suppressed.after, OwnershipMeaning::Suppressed);
    let kept = graph
        .resolve_report(&report("rep-1"), Some(DeliveryState::Submitted))
        .unwrap();
    assert_eq!(kept.after, OwnershipMeaning::Submitted);
    graph
        .resolve_report(&report("rep-1"), Some(DeliveryState::Suppressed))
        .unwrap();
    assert_eq!(
        graph.report_state(&report("rep-1")),
        Some(DeliveryState::Submitted)
    );
}

#[test]
fn suppressed_report_can_be_confirmed_or_left_in_place() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-child");
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::TerminalFailure,
            Initiator::Runtime,
        )
        .unwrap();
    let suppressed = graph
        .admit_report(report("rep-1"), &life("child"), &life("parent"))
        .unwrap();
    assert_eq!(suppressed.before, OwnershipMeaning::Absent);
    assert_eq!(suppressed.after, OwnershipMeaning::Suppressed);
    graph.resolve_report(&report("rep-1"), None).unwrap();
    assert_eq!(
        graph.report_state(&report("rep-1")),
        Some(DeliveryState::Suppressed)
    );
    graph
        .resolve_report(&report("rep-1"), Some(DeliveryState::Retained))
        .unwrap();
    assert_eq!(
        graph.report_state(&report("rep-1")),
        Some(DeliveryState::Retained)
    );
    assert_eq!(
        graph.resolve_report(&report("missing"), None),
        Err(OwnershipError::UnknownChild)
    );
}

#[test]
fn unconfirmed_lookup_confirms_a_partial_milestone_only() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Prepared,
            },
        )
        .unwrap();
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Attached)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Attached,
            },
        )
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::TaskAdmitted {
                receipt: receipt("task-1"),
            },
        )
        .unwrap();
    assert_eq!(
        graph.advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    assert_eq!(
        graph.advance_spawn(&spawn_id("missing"), SpawnProgress::Prepared),
        Err(OwnershipError::UnknownSpawn)
    );
}

#[test]
fn illegal_spawn_steps_and_dispatch_of_terminal_progress_are_refused() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    assert_eq!(
        graph.advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Prepared,
            },
        ),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    assert_eq!(
        graph.advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Draining {
                known: KnownMilestone::Attached,
            },
        ),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    assert_eq!(
        graph.advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::StartupFailed {
                known: KnownMilestone::Attached,
            },
        ),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Draining {
                known: KnownMilestone::Prepared,
            },
        )
        .unwrap();
    assert_eq!(
        graph.advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Ended {
                known: KnownMilestone::Reserved,
            },
        ),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::OwnerDisposed,
            Initiator::Runtime,
        )
        .unwrap();
    assert_eq!(
        graph.dispatch_after_prepare(&spawn_id("req-1")).unwrap(),
        Dispatch::Drain
    );
}

#[test]
fn child_pages_use_the_published_bound_and_a_cursor() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "a-child", "req-a");
    admit(&mut graph, "parent", "b-child", "req-b");
    assert_eq!(
        graph.children_page(&life("parent"), None, 0),
        Err(OwnershipError::PageLimit)
    );
    assert_eq!(
        graph.children_page(&life("parent"), None, MAX_READ_PAGE + 1),
        Err(OwnershipError::PageLimit)
    );
    assert_eq!(
        graph.children_page(&life("missing"), None, 1),
        Err(OwnershipError::ParentMissing)
    );
    let page = graph.children_page(&life("parent"), None, 1).unwrap();
    assert_eq!(page.children.len(), 1);
    assert_eq!(page.children[0].policy.revision(), "rev-1");
    assert!(page.next.is_some());
    let rest = graph
        .children_page(&life("parent"), page.next.as_ref(), 1)
        .unwrap();
    assert!(rest.next.is_none());
    assert_eq!(rest.children.len(), 1);
    let again = graph.snapshot();
    let restored = OwnershipGraph::restore(again.clone());
    assert!(restored.refusal().is_none());
    assert!(restored.recovery_records().is_empty());
    assert_eq!(restored.snapshot().spawns.len(), again.spawns.len());
}

#[test]
fn r5_illegal_history_stays_readable_and_refuses_dispatch() {
    let mut rows = OwnershipSnapshot::default();
    rows.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    rows.lifetimes.push(rows.lifetimes[0].clone());
    let mut duplicate = OwnershipGraph::restore(rows);
    assert_eq!(duplicate.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(
        duplicate.open_root(session("fresh"), life("fresh"), Initiator::Runtime),
        Err(OwnershipError::DispatchRefused)
    );

    let mut open_cause = OwnershipSnapshot::default();
    open_cause.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Open,
        close_operation: Some(close_id("x")),
        cause: Some(LifetimeCause::HostClose),
        initiator: Some(Initiator::Runtime),
        cascaded_from: None,
    });
    assert_eq!(
        OwnershipGraph::restore(open_cause).refusal(),
        Some(&OwnershipError::Contradictory)
    );

    let mut closing = OwnershipSnapshot::default();
    closing.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Closing,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    assert_eq!(
        OwnershipGraph::restore(closing).refusal(),
        Some(&OwnershipError::Contradictory)
    );

    let mut cycle = OwnershipSnapshot::default();
    cycle.lifetimes.push(LifetimeRow {
        lifetime_id: life("a"),
        session_id: session("a"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    cycle.lifetimes.push(LifetimeRow {
        lifetime_id: life("b"),
        session_id: session("b"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    cycle.spawns.push(SpawnRow {
        child_lifetime: life("b"),
        child_session: session("b"),
        binding: binding("a", "ab", "task"),
        progress: SpawnProgress::Reserved,
    });
    cycle.spawns.push(SpawnRow {
        child_lifetime: life("a"),
        child_session: session("a"),
        binding: SpawnBinding {
            parent_lifetime: life("b"),
            parent_session: session("b"),
            request_id: spawn_id("ba"),
            task_digest: digest("task"),
            policy: policy("rev-1"),
            model: Some(ModelChoice::new("codex", "gpt").unwrap()),
            origin: SpawnOrigin::Execution {
                execution: ExecutionId::new("exec").unwrap(),
                tool: None,
            },
        },
        progress: SpawnProgress::Reserved,
    });
    let mut cyclic = OwnershipGraph::restore(cycle);
    assert_eq!(cyclic.refusal(), Some(&OwnershipError::Cycle));
    assert_eq!(cyclic.lifetime_state(&life("a")), Some(LifetimeState::Open));
    assert_eq!(
        cyclic.admit_spawn(SpawnAdmission {
            child_lifetime: life("c"),
            child_session: session("c"),
            binding: binding("a", "more", "task"),
            live_room: true,
        }),
        Err(OwnershipError::Cycle)
    );
    assert_eq!(
        cyclic.advance_spawn(&spawn_id("ab"), SpawnProgress::Prepared),
        Err(OwnershipError::Cycle)
    );
    assert_eq!(
        cyclic.dispatch_after_prepare(&spawn_id("ab")),
        Err(OwnershipError::Cycle)
    );
    assert_eq!(
        cyclic.begin_close(
            &life("a"),
            close_id("close-a"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        ),
        Err(OwnershipError::Cycle)
    );
    assert_eq!(
        cyclic.apply_report(
            &life("a"),
            &close_id("close-a"),
            &life("b"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        ),
        Err(OwnershipError::Cycle)
    );
    assert_eq!(
        cyclic.admit_report(report("rep-a"), &life("b"), &life("a")),
        Err(OwnershipError::Cycle)
    );
    assert_eq!(
        cyclic.resolve_report(&report("rep-a"), None),
        Err(OwnershipError::Cycle)
    );

    let mut healthy = OwnershipGraph::new();
    root(&mut healthy, "parent");
    assert_eq!(
        healthy.dispatch_after_prepare(&spawn_id("missing")),
        Err(OwnershipError::UnknownSpawn)
    );

    let mut missing = OwnershipSnapshot::default();
    missing.spawns.push(SpawnRow {
        child_lifetime: life("child"),
        child_session: session("child"),
        binding: binding("missing", "req", "task"),
        progress: SpawnProgress::Reserved,
    });
    let missing_parent = OwnershipGraph::restore(missing);
    assert_eq!(
        missing_parent.refusal(),
        Some(&OwnershipError::ParentMissing)
    );
    assert!(missing_parent.spawn_progress(&spawn_id("req")).is_some());

    let mut foreign = OwnershipSnapshot::default();
    foreign.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    foreign.lifetimes.push(LifetimeRow {
        lifetime_id: life("child"),
        session_id: session("child"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    foreign.spawns.push(SpawnRow {
        child_lifetime: life("child"),
        child_session: session("child"),
        binding: SpawnBinding {
            parent_session: session("other"),
            ..binding("parent", "req", "task")
        },
        progress: SpawnProgress::Attached,
    });
    assert_eq!(
        OwnershipGraph::restore(foreign).refusal(),
        Some(&OwnershipError::ForeignParent)
    );

    let mut itself = OwnershipSnapshot::default();
    itself.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    itself.spawns.push(SpawnRow {
        child_lifetime: life("parent"),
        child_session: session("parent"),
        binding: binding("parent", "self", "task"),
        progress: SpawnProgress::Reserved,
    });
    assert_eq!(
        OwnershipGraph::restore(itself).refusal(),
        Some(&OwnershipError::SelfParent)
    );
}

#[test]
fn duplicate_spawn_and_report_rows_and_a_deeper_than_limit_chain_are_kept() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    let mut snapshot = graph.snapshot();
    snapshot.spawns.push(snapshot.spawns[0].clone());
    assert_eq!(
        OwnershipGraph::restore(snapshot).refusal(),
        Some(&OwnershipError::Contradictory)
    );

    let mut reports = OwnershipSnapshot::default();
    reports.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    reports.reports.push(ReportRow {
        report_id: report("rep"),
        child_lifetime: life("child"),
        parent_lifetime: life("parent"),
        state: DeliveryState::Retained,
    });
    reports.reports.push(reports.reports[0].clone());
    let restored = OwnershipGraph::restore(reports);
    assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));

    let mut deep = OwnershipSnapshot::default();
    deep.lifetimes.push(LifetimeRow {
        lifetime_id: life("n0"),
        session_id: session("n0"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    let mut parent = "n0".to_string();
    for index in 1..=5 {
        let name = format!("n{index}");
        deep.lifetimes.push(LifetimeRow {
            lifetime_id: life(&name),
            session_id: session(&name),
            state: LifetimeState::Open,
            close_operation: None,
            cause: None,
            initiator: None,
            cascaded_from: None,
        });
        deep.spawns.push(SpawnRow {
            child_lifetime: life(&name),
            child_session: session(&name),
            binding: binding(&parent, &format!("edge-{index}"), "task"),
            progress: SpawnProgress::Reserved,
        });
        parent = name;
    }
    let restored = OwnershipGraph::restore(deep);
    assert!(restored.refusal().is_none());
    let mut writable = restored;
    assert_eq!(
        writable.admit_spawn(SpawnAdmission {
            child_lifetime: life("past"),
            child_session: session("past"),
            binding: binding("n5", "past", "task"),
            live_room: true,
        }),
        Err(OwnershipError::DepthExceeded)
    );
}

#[test]
fn contradictory_child_and_missing_child_row_refuse_dispatch() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("child"),
            child_session: session("child-b"),
            binding: binding("parent", "req-2", "task"),
            live_room: true,
        }),
        Err(OwnershipError::Contradictory)
    );
    let mut snapshot = OwnershipSnapshot::default();
    snapshot.lifetimes.push(LifetimeRow {
        lifetime_id: life("parent"),
        session_id: session("parent"),
        state: LifetimeState::Open,
        close_operation: None,
        cause: None,
        initiator: None,
        cascaded_from: None,
    });
    snapshot.spawns.push(SpawnRow {
        child_lifetime: life("ghost"),
        child_session: session("ghost"),
        binding: binding("parent", "ghost", "task"),
        progress: SpawnProgress::Reserved,
    });
    let restored = OwnershipGraph::restore(snapshot);
    assert_eq!(restored.refusal(), Some(&OwnershipError::Contradictory));
    assert_eq!(
        restored.children_page(&life("parent"), None, 10),
        Err(OwnershipError::Contradictory)
    );
}

#[test]
fn unconfirmed_same_milestone_and_receipt_round_trip() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Attached)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::TaskAdmitted {
                receipt: receipt("task-1"),
            },
        )
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::TaskAdmitted {
                    receipt: receipt("task-1"),
                },
            },
        )
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::TaskAdmitted {
                receipt: receipt("task-1"),
            },
        )
        .unwrap();
    assert_eq!(
        graph.advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::TaskAdmitted {
                receipt: receipt("other"),
            },
        ),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    let model = graph.snapshot().spawns[0].binding.model.clone();
    assert!(model.is_none());
}

#[test]
fn apply_report_on_an_open_lifetime_is_stale() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    assert_eq!(
        graph.apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        ),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(
        graph.apply_report(
            &life("missing"),
            &close_id("close-1"),
            &life("missing"),
            PhysicalFact::Pending,
            EvidenceFact::Pending,
        ),
        Err(OwnershipError::ParentMissing)
    );
}

#[test]
fn begin_close_of_a_missing_lifetime_is_parent_missing() {
    let mut graph = OwnershipGraph::new();
    assert_eq!(
        graph.begin_close(
            &life("missing"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        ),
        Err(OwnershipError::ParentMissing)
    );
}

#[test]
fn second_physical_release_reuses_the_exact_first_observation() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close")),
        )
        .unwrap();
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Pending,
        )
        .unwrap();
    let again = graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    assert_eq!(again.before, OwnershipMeaning::Closing);
    assert_eq!(again.after, OwnershipMeaning::Closing);
    assert_eq!(
        again.close_detail,
        Some(CloseEvidenceDetail::ResourceObservation {
            physical: PhysicalFact::Released,
            provider_evidence: EvidenceFact::Pending,
        })
    );
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closing)
    );
    complete(&mut graph, &life("parent"));
    assert_eq!(
        graph.close_cause(&life("parent")),
        Some(&LifetimeCause::HostClose)
    );
}

#[test]
fn restored_settlement_stays_readable() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    graph
        .begin_close(
            &life("parent"),
            close_id("close-1"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("close")),
        )
        .unwrap();
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-1"),
            &life("parent"),
            PhysicalFact::Failed,
            EvidenceFact::Failed,
        )
        .unwrap();
    let restored = OwnershipGraph::restore(graph.snapshot());
    assert!(restored.refusal().is_none());
    assert_eq!(
        restored.physical(&life("parent"), &life("parent")),
        Some(PhysicalFact::Failed)
    );
}

#[test]
fn unconfirmed_spawn_returns_to_its_milestone_or_joins_a_drain() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-1");
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    assert_eq!(
        graph.advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Draining {
                known: KnownMilestone::Prepared,
            },
        ),
        Err(OwnershipError::IllegalSpawnProgress)
    );
    graph
        .advance_spawn(&spawn_id("req-1"), SpawnProgress::Reserved)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-1"),
            SpawnProgress::Draining {
                known: KnownMilestone::Reserved,
            },
        )
        .unwrap();

    admit(&mut graph, "parent", "prepared", "req-prepared");
    graph
        .advance_spawn(&spawn_id("req-prepared"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-prepared"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Prepared,
            },
        )
        .unwrap();
    graph
        .advance_spawn(&spawn_id("req-prepared"), SpawnProgress::Prepared)
        .unwrap();

    admit(&mut graph, "parent", "attached", "req-attached");
    graph
        .advance_spawn(&spawn_id("req-attached"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .advance_spawn(&spawn_id("req-attached"), SpawnProgress::Attached)
        .unwrap();
    graph
        .advance_spawn(
            &spawn_id("req-attached"),
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Attached,
            },
        )
        .unwrap();
    graph
        .advance_spawn(&spawn_id("req-attached"), SpawnProgress::Attached)
        .unwrap();
}

#[test]
fn a_closed_child_is_omitted_when_the_parent_seals() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "req-child");
    graph
        .begin_close(
            &life("child"),
            close_id("close-child"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("child")),
        )
        .unwrap();
    graph
        .apply_report(
            &life("child"),
            &close_id("close-child"),
            &life("child"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("child"));
    let admission = graph
        .begin_close(
            &life("parent"),
            close_id("close-parent"),
            LifetimeCause::HostClose,
            Initiator::Host(actor("parent")),
        )
        .unwrap();
    assert!(!admission.targets.contains(&life("child")));
    assert!(admission.targets.contains(&life("parent")));
    graph
        .apply_report(
            &life("parent"),
            &close_id("close-parent"),
            &life("parent"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("parent"));
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        graph.lifetime_state(&life("child")),
        Some(LifetimeState::Closed)
    );
}

fn row(name: &str, state: LifetimeState) -> LifetimeRow {
    let sealed = state != LifetimeState::Open;
    LifetimeRow {
        lifetime_id: life(name),
        session_id: session(name),
        state,
        close_operation: sealed.then_some(close_id("close-1")),
        cause: sealed.then_some(LifetimeCause::HostClose),
        initiator: sealed.then_some(Initiator::Host(actor("close"))),
        cascaded_from: None,
    }
}

fn child_spawn(parent: &str, child: &str, request: &str) -> SpawnRow {
    SpawnRow {
        child_lifetime: life(child),
        child_session: session(child),
        binding: binding(parent, request, request),
        progress: SpawnProgress::Reserved,
    }
}

#[test]
fn r3_an_open_descendant_of_a_closing_ancestor_joins_that_close() {
    let mut snapshot = OwnershipSnapshot::default();
    snapshot
        .lifetimes
        .push(row("parent", LifetimeState::Closing));
    snapshot.lifetimes.push(row("child", LifetimeState::Open));
    snapshot.lifetimes.push(row("grand", LifetimeState::Open));
    snapshot
        .spawns
        .push(child_spawn("parent", "child", "child-req"));
    snapshot
        .spawns
        .push(child_spawn("child", "grand", "grand-req"));
    let mut graph = OwnershipGraph::restore(snapshot);
    assert!(graph.refusal().is_none());
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        graph.close_cause(&life("parent")),
        Some(&LifetimeCause::HostClose)
    );
    assert!(graph.cascaded_from(&life("parent")).is_none());
    assert_eq!(
        graph.lifetime_state(&life("child")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        graph.lifetime_state(&life("grand")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(graph.cascaded_from(&life("child")), Some(&life("parent")));
    assert_eq!(graph.cascaded_from(&life("grand")), Some(&life("child")));
    assert_eq!(
        graph.close_cause(&life("child")),
        Some(&LifetimeCause::HostClose)
    );
    assert_eq!(
        graph
            .close_operation(&life("grand"))
            .map(CloseOperationId::as_str),
        Some("close-1")
    );
    assert_eq!(graph.recovery_records().len(), 2);
    assert!(graph.recovery_records().iter().all(|record| {
        record.before == OwnershipMeaning::Open && record.after == OwnershipMeaning::Closing
    }));
    let joined = graph
        .begin_close(
            &life("parent"),
            close_id("close-again"),
            LifetimeCause::OwnerDisposed,
            Initiator::Runtime,
        )
        .unwrap();
    assert!(joined.joined);
    assert_eq!(
        graph.close_cause(&life("parent")),
        Some(&LifetimeCause::HostClose)
    );
    assert!(joined.targets.contains(&life("child")));
    assert!(joined.targets.contains(&life("grand")));
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("extra"),
            child_session: session("extra"),
            binding: binding("child", "extra-req", "extra"),
            live_room: true,
        }),
        Err(OwnershipError::ParentClosing)
    );

    let mut direct = OwnershipSnapshot::default();
    direct.lifetimes.push(row("parent", LifetimeState::Closing));
    let mut child = row("child", LifetimeState::Closing);
    child.cause = Some(LifetimeCause::OwnerDisposed);
    child.close_operation = Some(close_id("close-child"));
    child.initiator = Some(Initiator::Runtime);
    child.cascaded_from = None;
    direct.lifetimes.push(child);
    direct.lifetimes.push(row("grand", LifetimeState::Open));
    direct
        .spawns
        .push(child_spawn("parent", "child", "child-req"));
    direct
        .spawns
        .push(child_spawn("child", "grand", "grand-req"));
    let graph = OwnershipGraph::restore(direct);
    assert!(graph.refusal().is_none());
    assert_eq!(
        graph.close_cause(&life("child")),
        Some(&LifetimeCause::OwnerDisposed)
    );
    assert_eq!(
        graph
            .close_operation(&life("child"))
            .map(CloseOperationId::as_str),
        Some("close-child")
    );
    assert_eq!(
        graph.close_cause(&life("parent")),
        Some(&LifetimeCause::HostClose)
    );
    assert_eq!(
        graph.lifetime_state(&life("grand")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        graph.close_cause(&life("grand")),
        Some(&LifetimeCause::OwnerDisposed)
    );
    assert_eq!(graph.cascaded_from(&life("grand")), Some(&life("child")));
    assert_eq!(graph.recovery_records().len(), 1);
    assert_eq!(graph.recovery_records()[0].parent_lifetime, life("grand"));
}

#[test]
fn r5_an_open_child_under_a_closed_ancestor_stays_readable() {
    let mut snapshot = OwnershipSnapshot::default();
    snapshot
        .lifetimes
        .push(row("closed-root", LifetimeState::Closed));
    snapshot
        .lifetimes
        .push(row("closed-child", LifetimeState::Open));
    snapshot
        .lifetimes
        .push(row("closing", LifetimeState::Closing));
    snapshot.lifetimes.push(row("gap", LifetimeState::Open));
    snapshot
        .spawns
        .push(child_spawn("closed-root", "closed-child", "closed-req"));
    snapshot
        .spawns
        .push(child_spawn("closing", "gap", "gap-req"));
    let mut graph = OwnershipGraph::restore(snapshot);
    assert_eq!(graph.refusal(), Some(&OwnershipError::Contradictory));
    assert!(graph.recovery_records().is_empty());
    assert_eq!(
        graph.lifetime_state(&life("closed-child")),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        graph.lifetime_state(&life("gap")),
        Some(LifetimeState::Open)
    );
    assert_eq!(
        graph.lifetime_state(&life("closed-root")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        graph.admit_spawn(SpawnAdmission {
            child_lifetime: life("extra"),
            child_session: session("extra"),
            binding: binding("gap", "extra-req", "extra"),
            live_room: true,
        }),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(
        graph.begin_close(
            &life("closing"),
            close_id("close-again"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        ),
        Err(OwnershipError::DispatchRefused)
    );
}

#[test]
fn row_20_private_root_discard_preserves_neighbors_closed_history_reports_and_recovery() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "private");
    root(&mut graph, "neighbor");
    root(&mut graph, "history");
    admit(&mut graph, "neighbor", "child", "request");
    graph
        .advance_spawn(&spawn_id("request"), SpawnProgress::Prepared)
        .unwrap();
    graph
        .admit_report(report("retained-report"), &life("child"), &life("neighbor"))
        .unwrap();
    graph
        .begin_close(
            &life("history"),
            close_id("history-close"),
            LifetimeCause::Deletion,
            Initiator::Host(actor("history-close")),
        )
        .unwrap();
    graph
        .apply_report(
            &life("history"),
            &close_id("history-close"),
            &life("history"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("history"));
    graph
        .begin_close(
            &life("neighbor"),
            close_id("neighbor-close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let mut expected = graph.snapshot();
    expected
        .lifetimes
        .retain(|r| r.lifetime_id != life("private"));
    graph.discard_private_root(&life("private")).unwrap();
    assert_eq!(graph.snapshot(), expected);
    assert!(graph.recovery_records().is_empty());
    // A live rollback via restore would change this interrupted-cascade neighbor.
    // Preserve an intentionally open descendant without invoking restoration.
    let mut snapshot = graph.snapshot();
    snapshot
        .lifetimes
        .iter_mut()
        .find(|r| r.lifetime_id == life("child"))
        .unwrap()
        .state = LifetimeState::Open;
    snapshot
        .lifetimes
        .iter_mut()
        .find(|r| r.lifetime_id == life("child"))
        .unwrap()
        .close_operation = None;
    snapshot
        .lifetimes
        .iter_mut()
        .find(|r| r.lifetime_id == life("child"))
        .unwrap()
        .cause = None;
    snapshot
        .lifetimes
        .iter_mut()
        .find(|r| r.lifetime_id == life("child"))
        .unwrap()
        .initiator = None;
    snapshot
        .lifetimes
        .iter_mut()
        .find(|r| r.lifetime_id == life("child"))
        .unwrap()
        .cascaded_from = None;
    let mut restored = OwnershipGraph::restore(snapshot);
    let recovery = restored.recovery_records().to_vec();
    assert!(!recovery.is_empty());
    root(&mut restored, "private-again");
    let mut expected = restored.snapshot();
    expected
        .lifetimes
        .retain(|r| r.lifetime_id != life("private-again"));
    restored
        .discard_private_root(&life("private-again"))
        .unwrap();
    assert_eq!(restored.snapshot(), expected);
    assert_eq!(restored.recovery_records(), recovery);
}

#[test]
fn active_root_query_excludes_child_and_closed_history_and_discard_refuses_dependents() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "root");
    admit(&mut graph, "root", "child", "request");
    assert_eq!(
        graph.root_lifetime_for_session(&session("root")),
        Some(&life("root"))
    );
    assert!(graph.root_lifetime_for_session(&session("child")).is_none());
    assert_eq!(
        graph.discard_private_root(&life("root")),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(
        graph.discard_private_root(&life("child")),
        Err(OwnershipError::StaleOutcome)
    );
    root(&mut graph, "history");
    graph
        .begin_close(
            &life("history"),
            close_id("close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    assert_eq!(
        graph.root_lifetime_for_session(&session("history")),
        Some(&life("history"))
    );
    let token = graph
        .note_unbound_root(&life("history"), &close_id("close"))
        .unwrap();
    graph
        .acknowledge_unbound_root(token, EvidenceFact::Acknowledged)
        .unwrap();
    complete(&mut graph, &life("history"));
    assert!(graph
        .root_lifetime_for_session(&session("history"))
        .is_none());
    assert_eq!(
        graph.discard_private_root(&life("history")),
        Err(OwnershipError::StaleOutcome)
    );
    assert_eq!(
        graph
            .note_unbound_root(&life("child"), &close_id("close"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
}

#[test]
fn unbound_absence_token_refuses_child_closed_history_and_stale_completion() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "root");
    graph
        .begin_close(
            &life("root"),
            close_id("close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let before = graph.snapshot();
    assert_eq!(
        graph
            .note_unbound_root(&life("root"), &close_id("wrong-close"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
    assert_eq!(graph.snapshot(), before);
    assert_eq!(graph.physical(&life("root"), &life("root")), None);
    assert_eq!(
        graph.close_cause(&life("root")),
        Some(&LifetimeCause::HostClose)
    );
    let token = graph
        .note_unbound_root(&life("root"), &close_id("close"))
        .unwrap();
    assert_eq!(
        graph.apply_report(
            &life("root"),
            &close_id("close"),
            &life("root"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged
        ),
        Err(OwnershipError::StaleOutcome)
    );
    graph
        .acknowledge_unbound_root(token, EvidenceFact::Acknowledged)
        .unwrap();
    complete(&mut graph, &life("root"));
    assert_eq!(
        graph
            .note_unbound_root(&life("root"), &close_id("close"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
    assert_eq!(
        graph.snapshot().settlements[0].evidence,
        EvidenceFact::Acknowledged
    );
}

#[test]
fn unbound_absence_admission_refuses_child_and_stale_operation_without_mutating_history() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "request");
    graph
        .begin_close(
            &life("parent"),
            close_id("original"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let before = graph.snapshot();
    assert_eq!(
        graph
            .note_unbound_root(&life("child"), &close_id("original"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
    assert_eq!(graph.snapshot(), before);
    assert_eq!(
        graph
            .note_unbound_root(&life("parent"), &close_id("stale"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
    assert_eq!(graph.snapshot(), before);
    let token = graph
        .note_unbound_root(&life("parent"), &close_id("original"))
        .unwrap();
    graph
        .acknowledge_unbound_root(token, EvidenceFact::Failed)
        .unwrap();
    let rejected = graph.snapshot();
    assert_eq!(
        graph
            .note_unbound_root(&life("parent"), &close_id("stale-again"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
    assert_eq!(graph.snapshot(), rejected);
    assert_eq!(
        graph.physical(&life("parent"), &life("parent")),
        Some(PhysicalFact::Released)
    );
    assert_eq!(
        graph.lifetime_state(&life("parent")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(rejected.settlements[0].evidence, EvidenceFact::Failed);
}

#[test]
fn unbound_absence_completion_refuses_restored_history_despite_matching_token() {
    let mut graph = OwnershipGraph::new();
    root(&mut graph, "root");
    graph
        .begin_close(
            &life("root"),
            close_id("close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let token = graph
        .note_unbound_root(&life("root"), &close_id("close"))
        .unwrap();
    let mut history = graph.snapshot();
    history.lifetimes.push(history.lifetimes[0].clone());
    let mut refused = OwnershipGraph::restore(history);
    assert_eq!(refused.refusal(), Some(&OwnershipError::Contradictory));
    let before = refused.snapshot();
    assert_eq!(
        refused.acknowledge_unbound_root(token, EvidenceFact::Acknowledged),
        Err(OwnershipError::DispatchRefused)
    );
    assert_eq!(refused.snapshot(), before);
    assert_eq!(
        refused.lifetime_state(&life("root")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        refused.physical(&life("root"), &life("root")),
        Some(PhysicalFact::Released)
    );
    assert_eq!(before.settlements[0].evidence, EvidenceFact::Pending);
    assert_eq!(
        refused.close_cause(&life("root")),
        Some(&LifetimeCause::HostClose)
    );
}

#[test]
fn sealed_progress_retains_actual_receipt_without_inventing_permission_or_terminal_changes() {
    let mut graph = OwnershipGraph::new();
    assert_eq!(
        graph.sealed_spawn_progress(&spawn_id("missing"), &SpawnProgress::Reserved),
        None
    );
    root(&mut graph, "parent");
    admit(&mut graph, "parent", "child", "request");
    let request = spawn_id("request");
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Reserved),
        None
    );
    graph
        .advance_spawn(&request, SpawnProgress::Prepared)
        .unwrap();
    graph
        .advance_spawn(&request, SpawnProgress::Attached)
        .unwrap();
    let actual = SpawnProgress::TaskAdmitted {
        receipt: receipt("actual"),
    };
    graph.advance_spawn(&request, actual.clone()).unwrap();
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Attached),
        None
    );
    graph
        .begin_close(
            &life("child"),
            close_id("stop"),
            LifetimeCause::TerminalFailure,
            Initiator::Runtime,
        )
        .unwrap();
    assert_eq!(graph.sealed_spawn_progress(&request, &actual), None);
    let pending = SpawnProgress::Unconfirmed {
        known: actual.known(),
    };
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Attached),
        Some(pending.clone())
    );
    graph.advance_spawn(&request, pending.clone()).unwrap();
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Attached),
        Some(pending)
    );
    let draining = SpawnProgress::Draining {
        known: actual.known(),
    };
    graph.advance_spawn(&request, draining.clone()).unwrap();
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Attached),
        Some(draining)
    );
    let ended = SpawnProgress::Ended {
        known: actual.known(),
    };
    graph.advance_spawn(&request, ended.clone()).unwrap();
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Attached),
        Some(ended.clone())
    );
    graph
        .apply_report(
            &life("child"),
            &close_id("stop"),
            &life("child"),
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    complete(&mut graph, &life("child"));
    assert_eq!(
        graph.lifetime_state(&life("child")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        graph.sealed_spawn_progress(&request, &SpawnProgress::Attached),
        Some(ended)
    );
    assert_eq!(
        graph.spawn_progress(&request),
        Some(&SpawnProgress::Ended {
            known: actual.known()
        })
    );
}
