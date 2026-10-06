//! Ownership graph rows from ADR 329. Each test names the table row it locks.
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
    assert!(CloseOperationId::new("ok-1").is_ok());
    assert!(matches!(
        TaskDigest::new("abc"),
        Err(OwnershipError::InvalidTaskDigest)
    ));
    assert!(matches!(
        TaskDigest::new("A".repeat(64)),
        Err(OwnershipError::InvalidTaskDigest)
    ));
    assert!(digest("b").as_str().len() == 64);
    assert!(matches!(
        ApprovalPolicy::new("  ", "ask", "rev"),
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
    let choice = ModelChoice::new("codex", "gpt").unwrap();
    assert_eq!(choice.provider(), "codex");
    assert_eq!(choice.model(), "gpt");
    assert!(matches!(
        HostActor::new("p", " ", "r"),
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
fn second_physical_release_records_the_previous_release_as_before() {
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
    assert_eq!(again.before, OwnershipMeaning::Closed);
    assert_eq!(again.after, OwnershipMeaning::Closed);
    assert_eq!(
        graph.close_cause(&life("parent")),
        Some(&LifetimeCause::HostClose)
    );
}
