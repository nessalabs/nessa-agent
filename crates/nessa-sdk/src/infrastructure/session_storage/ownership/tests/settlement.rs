//! Actual SQLite recovery of exact close debt and rejection of contradictory bodies.
use super::*;
use crate::domain::agent_execution::subagents::{
    AbsenceProof, ApprovalPolicy, CloseEvidenceDetail, CloseOperationId, EvidenceFact, HostActor,
    LifetimeCause, OwnershipError, OwnershipEvidence, PhysicalFact, SettlementProof,
    SpawnAdmission, SpawnBinding, SpawnOrigin, SpawnRequestId, TaskDigest,
};
use serde_json::{json, Value};

fn lifetime(name: &str) -> AgentLifetimeId {
    AgentLifetimeId::new(name).unwrap()
}

fn operation(name: &str) -> CloseOperationId {
    CloseOperationId::new(name).unwrap()
}

fn open_root(graph: &mut OwnershipGraph) {
    let _opened = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            lifetime("root"),
            Initiator::Runtime,
        )
        .unwrap();
}

fn closing_resource() -> OwnershipGraph {
    let mut graph = OwnershipGraph::new();
    open_root(&mut graph);
    let _close = graph
        .begin_close(
            &lifetime("root"),
            operation("root-close"),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    graph
}

fn resource_report(
    graph: &mut OwnershipGraph,
    physical: PhysicalFact,
    provider: EvidenceFact,
) -> OwnershipEvidence {
    graph
        .apply_report(
            &lifetime("root"),
            &operation("root-close"),
            &lifetime("root"),
            physical,
            provider,
        )
        .unwrap()
}

fn acknowledge_debt(graph: &mut OwnershipGraph, name: &str) {
    for record in graph.pending_close_evidence(&lifetime(name)) {
        graph
            .acknowledge_observation(&record, EvidenceFact::Acknowledged)
            .unwrap();
    }
}

fn complete(graph: &mut OwnershipGraph, name: &str) {
    acknowledge_debt(graph, name);
    let id = lifetime(name);
    let close = graph.close_operation(&id).cloned().unwrap();
    let token = graph.prepare_completion(&id, &close).unwrap();
    graph
        .acknowledge_completion(&token, EvidenceFact::Acknowledged)
        .unwrap();
    // A later failed attempt cannot erase an already acknowledged exact token.
    graph
        .acknowledge_completion(&token, EvidenceFact::Failed)
        .unwrap();
}

async fn reopen_snapshot(path: &Path, snapshot: &OwnershipSnapshot) -> OwnershipGraph {
    let store = SqliteOwnershipStore::open(path).unwrap();
    store.write(snapshot).await.unwrap();
    drop(store);
    let reopened = SqliteOwnershipStore::open(path).unwrap();
    let loaded = reopened.read().await.unwrap();
    assert_eq!(&loaded, snapshot);
    let restored = OwnershipGraph::restore(loaded);
    assert_eq!(restored.refusal(), None);
    restored
}

fn stored_body(path: &Path) -> String {
    let connection = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        1
    );
    connection
        .query_row(
            "SELECT body FROM ownership_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

async fn reject_body_without_rewrite(path: &Path, body: &str, case: &str) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "UPDATE ownership_snapshot SET body = ?1 WHERE id = 1",
            [body],
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(path).unwrap();
    let store = SqliteOwnershipStore::open(path).unwrap();
    assert_eq!(store.read().await, Err(PortFailure::Rejected), "{case}");
    drop(store);
    assert_eq!(stored_body(path), body, "body changed for {case}");
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "file changed for {case}"
    );
}

#[tokio::test]
async fn mixed_close_debt_and_later_release_witness_survive_sqlite_reopen() {
    let directory = private_directory();
    let path = directory.path().join("private/settlement.sqlite3");
    let mut graph = closing_resource();
    let pending = resource_report(&mut graph, PhysicalFact::Pending, EvidenceFact::Failed);
    graph
        .acknowledge_observation(&pending, EvidenceFact::Acknowledged)
        .unwrap();
    graph
        .acknowledge_observation(&pending, EvidenceFact::Failed)
        .unwrap();
    let failed = resource_report(&mut graph, PhysicalFact::Failed, EvidenceFact::Acknowledged);
    graph
        .acknowledge_observation(&failed, EvidenceFact::Failed)
        .unwrap();
    let released = resource_report(&mut graph, PhysicalFact::Released, EvidenceFact::Failed);
    let mut restored = reopen_snapshot(&path, &graph.snapshot()).await;
    assert_eq!(
        restored.pending_close_evidence(&lifetime("root")),
        vec![failed.clone(), released.clone()]
    );
    assert_eq!(
        restored.lifetime_state(&lifetime("root")),
        Some(LifetimeState::Closing)
    );
    assert_eq!(
        restored.physical(&lifetime("root"), &lifetime("root")),
        Some(PhysicalFact::Released)
    );
    acknowledge_debt(&mut restored, "root");
    assert_eq!(
        restored
            .prepare_completion(&lifetime("root"), &operation("root-close"))
            .unwrap_err(),
        OwnershipError::StaleOutcome,
        "an earlier Failed provider acknowledgement does not witness Released"
    );
    let same = resource_report(
        &mut restored,
        PhysicalFact::Released,
        EvidenceFact::Acknowledged,
    );
    assert_eq!(same, released);
    let mut restored = reopen_snapshot(&path, &restored.snapshot()).await;
    assert!(restored
        .pending_close_evidence(&lifetime("root"))
        .is_empty());
    assert_eq!(
        restored.lifetime_state(&lifetime("root")),
        Some(LifetimeState::Closing)
    );
    let snapshot = restored.snapshot();
    let SettlementProof::Resource(slots) = &snapshot.settlements[0].proof else {
        panic!("expected actual resource observations");
    };
    assert_eq!(slots.iter().flatten().count(), 3);
    assert_eq!(slots[0].as_ref().unwrap().record(), &pending);
    assert_eq!(
        slots[0].as_ref().unwrap().acknowledgement(),
        EvidenceFact::Acknowledged
    );
    assert_eq!(slots[2].as_ref().unwrap().record(), &released);
    assert!(slots[2].as_ref().unwrap().provider_acknowledged());
    complete(&mut restored, "root");
    let closed = reopen_snapshot(&path, &restored.snapshot()).await;
    assert_eq!(
        closed.lifetime_state(&lifetime("root")),
        Some(LifetimeState::Closed)
    );
    assert_eq!(closed.snapshot().close_completions.len(), 1);
}

fn independently_closing_absent_children() -> OwnershipGraph {
    let mut graph = OwnershipGraph::new();
    open_root(&mut graph);
    for (child, request, proof) in [
        (
            "prepared-child",
            "prepare-request",
            AbsenceProof::PreparationRejectedWithoutOwner(
                SpawnRequestId::new("prepare-request").unwrap(),
            ),
        ),
        (
            "admitted-child",
            "admit-request",
            AbsenceProof::AdmissionFailedBeforeFactory(
                SpawnRequestId::new("admit-request").unwrap(),
            ),
        ),
    ] {
        let _admitted = graph
            .admit_spawn(SpawnAdmission {
                child_lifetime: lifetime(child),
                child_session: SessionId::new(child).unwrap(),
                binding: SpawnBinding {
                    parent_lifetime: lifetime("root"),
                    parent_session: SessionId::new("root-session").unwrap(),
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
        let _close = graph
            .begin_close(
                &lifetime(child),
                operation(child),
                LifetimeCause::TerminalFailure,
                Initiator::Runtime,
            )
            .unwrap();
        let _absence = graph
            .note_absence(&lifetime(child), &operation(child), &lifetime(child), proof)
            .unwrap();
    }
    let _close = graph
        .begin_close(
            &lifetime("root"),
            operation("root-close"),
            LifetimeCause::Deletion,
            Initiator::Runtime,
        )
        .unwrap();
    let _absence = graph
        .note_unbound_root(&lifetime("root"), &operation("root-close"))
        .unwrap();
    graph
}

#[tokio::test]
async fn actual_absence_and_independent_completions_survive_sqlite_reopen() {
    let directory = private_directory();
    let path = directory.path().join("private/absence.sqlite3");
    let graph = independently_closing_absent_children();
    let mut restored = reopen_snapshot(&path, &graph.snapshot()).await;
    for name in ["root", "prepared-child", "admitted-child"] {
        assert!(restored.has_absence(&lifetime(name)));
        assert_eq!(
            restored.pending_close_evidence(&lifetime(name)),
            graph.pending_close_evidence(&lifetime(name))
        );
    }
    acknowledge_debt(&mut restored, "root");
    assert_eq!(
        restored
            .prepare_completion(&lifetime("root"), &operation("root-close"))
            .unwrap_err(),
        OwnershipError::StaleOutcome
    );
    complete(&mut restored, "prepared-child");
    let mut restored = reopen_snapshot(&path, &restored.snapshot()).await;
    assert_eq!(
        restored.lifetime_state(&lifetime("root")),
        Some(LifetimeState::Closing)
    );
    complete(&mut restored, "admitted-child");
    complete(&mut restored, "root");
    let restored = reopen_snapshot(&path, &restored.snapshot()).await;
    assert_eq!(restored.snapshot().close_completions.len(), 3);
    assert_eq!(
        restored.close_cause(&lifetime("root")),
        Some(&LifetimeCause::Deletion)
    );
    for child in ["prepared-child", "admitted-child"] {
        assert_eq!(
            restored.close_owner(&lifetime(child)),
            Some(lifetime(child))
        );
        assert_eq!(
            restored.close_operation(&lifetime(child)),
            Some(&operation(child))
        );
        assert_eq!(
            restored.close_cause(&lifetime(child)),
            Some(&LifetimeCause::TerminalFailure)
        );
        assert_eq!(
            restored.lifetime_state(&lifetime(child)),
            Some(LifetimeState::Closed)
        );
    }
    for row in restored.snapshot().settlements {
        let SettlementProof::Absence(absence) = row.proof else {
            panic!("expected actual absence");
        };
        assert_eq!(
            absence.record().close_detail,
            Some(CloseEvidenceDetail::Absence(absence.proof().clone()))
        );
    }
}

#[tokio::test]
async fn contradictory_resource_debt_is_rejected_without_sqlite_rewrite() {
    let directory = private_directory();
    let path = directory.path().join("private/resource-rejections.sqlite3");
    let mut graph = closing_resource();
    for physical in [
        PhysicalFact::Pending,
        PhysicalFact::Failed,
        PhysicalFact::Released,
    ] {
        let record = resource_report(&mut graph, physical, EvidenceFact::Acknowledged);
        graph
            .acknowledge_observation(&record, EvidenceFact::Failed)
            .unwrap();
    }
    let _valid = reopen_snapshot(&path, &graph.snapshot()).await;
    let valid: Value = serde_json::from_str(&stored_body(&path)).unwrap();
    let cases = [
        "foreign owner",
        "foreign target",
        "foreign operation",
        "different cause",
        "different actor",
        "wrong physical slot",
        "provider witness erased",
        "summary debt erased",
        "summary physical rewound",
        "duplicate settlement",
        "Closed without Completion",
    ];
    for (index, case) in cases.iter().enumerate() {
        let mut invalid = valid.clone();
        match index {
            0 => {
                invalid["settlements"][0]["proof"]["observations"][2]["record"]["parent"] =
                    json!("foreign")
            }
            1 => invalid["settlements"][0]["target"] = json!("foreign"),
            2 => {
                invalid["settlements"][0]["proof"]["observations"][2]["record"]["operation"] =
                    json!("other-close")
            }
            3 => {
                invalid["settlements"][0]["proof"]["observations"][2]["record"]["cause"] =
                    json!("deletion")
            }
            4 => {
                invalid["settlements"][0]["proof"]["observations"][2]["record"]["initiator"] = json!({"kind":"host","principal_id":"person","surface_id":"desktop","request_id":"other"})
            }
            5 => {
                invalid["settlements"][0]["proof"]["observations"][2]["record"]["detail"]
                    ["physical"] = json!("failed")
            }
            6 => {
                invalid["settlements"][0]["proof"]["observations"][2]["provider_acknowledged"] =
                    json!(false)
            }
            7 => invalid["settlements"][0]["evidence"] = json!("acknowledged"),
            8 => invalid["settlements"][0]["physical"] = json!("failed"),
            9 => {
                let row = invalid["settlements"][0].clone();
                invalid["settlements"].as_array_mut().unwrap().push(row);
            }
            _ => invalid["lifetimes"][0]["state"] = json!("closed"),
        }
        reject_body_without_rewrite(&path, &serde_json::to_string(&invalid).unwrap(), case).await;
    }
}

#[tokio::test]
async fn contradictory_absence_and_completion_are_rejected_without_sqlite_rewrite() {
    let directory = private_directory();
    let path = directory.path().join("private/absence-rejections.sqlite3");
    let mut graph = independently_closing_absent_children();
    for name in ["prepared-child", "admitted-child", "root"] {
        complete(&mut graph, name);
    }
    let _valid = reopen_snapshot(&path, &graph.snapshot()).await;
    let valid: Value = serde_json::from_str(&stored_body(&path)).unwrap();
    let child_slot = valid["settlements"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row["target"] == "prepared-child")
        .unwrap();
    let root_completion = valid["close_completions"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row["root"] == "root")
        .unwrap();
    let cases = [
        "foreign absence request",
        "absence detail disagrees",
        "synthetic provider field",
        "Completion is observation",
        "Completion foreign operation",
        "Completion duplicate",
        "Closed missing Completion",
        "Closed unacknowledged Completion",
        "Closed missing debt Ack",
        "parent takes independent child",
        "absence wrong target kind",
    ];
    for (index, case) in cases.iter().enumerate() {
        let mut invalid = valid.clone();
        match index {
            0 => {
                invalid["settlements"][child_slot]["proof"]["proof"]["request"] =
                    json!("other-request")
            }
            1 => {
                invalid["settlements"][child_slot]["proof"]["record"]["detail"]["proof"]
                    ["request"] = json!("other-request")
            }
            2 => invalid["settlements"][child_slot]["proof"]["provider_acknowledged"] = json!(true),
            3 => {
                invalid["close_completions"][root_completion]["record"]["detail"] = json!({"kind":"ResourceObservation","physical":"released","provider_evidence":"acknowledged"})
            }
            4 => {
                invalid["close_completions"][root_completion]["record"]["operation"] =
                    json!("prepared-child")
            }
            5 => {
                let row = invalid["close_completions"][root_completion].clone();
                invalid["close_completions"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
            6 => {
                invalid["close_completions"]
                    .as_array_mut()
                    .unwrap()
                    .remove(root_completion);
            }
            7 => invalid["close_completions"][root_completion]["acknowledgement"] = json!("failed"),
            8 => invalid["settlements"][child_slot]["proof"]["acknowledgement"] = json!("failed"),
            9 => invalid["settlements"][child_slot]["close_lifetime"] = json!("root"),
            _ => {
                invalid["settlements"][child_slot]["proof"]["proof"] =
                    json!({"kind":"NeverTransferredRoot"});
                invalid["settlements"][child_slot]["proof"]["record"]["detail"]["proof"] =
                    json!({"kind":"NeverTransferredRoot"});
            }
        }
        reject_body_without_rewrite(&path, &serde_json::to_string(&invalid).unwrap(), case).await;
    }
}

#[tokio::test]
async fn current_empty_shape_is_accepted_and_proofless_old_body_is_rejected_unchanged() {
    let directory = private_directory();
    let path = directory.path().join("private/body-contract.sqlite3");
    let _empty = reopen_snapshot(&path, &OwnershipSnapshot::default()).await;
    let current = stored_body(&path);
    let mut old_empty: Value = serde_json::from_str(&current).unwrap();
    old_empty
        .as_object_mut()
        .unwrap()
        .remove("close_completions");
    reject_body_without_rewrite(
        &path,
        &serde_json::to_string(&old_empty).unwrap(),
        "missing current Completion collection",
    )
    .await;
    let mut graph = closing_resource();
    let _released = resource_report(
        &mut graph,
        PhysicalFact::Released,
        EvidenceFact::Acknowledged,
    );
    acknowledge_debt(&mut graph, "root");
    let _closing = reopen_snapshot(&path, &graph.snapshot()).await;
    let mut proofless: Value = serde_json::from_str(&stored_body(&path)).unwrap();
    proofless["settlements"][0]
        .as_object_mut()
        .unwrap()
        .remove("proof");
    reject_body_without_rewrite(
        &path,
        &serde_json::to_string(&proofless).unwrap(),
        "proofless settlement despite current outer shape",
    )
    .await;
    proofless
        .as_object_mut()
        .unwrap()
        .remove("close_completions");
    reject_body_without_rewrite(
        &path,
        &serde_json::to_string(&proofless).unwrap(),
        "proofless historical body",
    )
    .await;
}

#[tokio::test]
async fn empty_resource_proof_is_rejected_unchanged_with_matching_pending_summary() {
    let directory = private_directory();
    let path = directory.path().join("private/empty-proof.sqlite3");
    let mut graph = closing_resource();
    let exact = resource_report(&mut graph, PhysicalFact::Pending, EvidenceFact::Pending);
    let restored = reopen_snapshot(&path, &graph.snapshot()).await;
    assert_eq!(
        restored.pending_close_evidence(&lifetime("root")),
        vec![exact]
    );
    assert_eq!(
        restored.lifetime_state(&lifetime("root")),
        Some(LifetimeState::Closing)
    );
    assert!(restored.snapshot().close_completions.is_empty());
    let mut invalid: Value = serde_json::from_str(&stored_body(&path)).unwrap();
    // Preserve valid owner/operation/cause/actor and the exact three-slot shape.
    // The coarse facts also match an unobserved Pending target, so this case
    // isolates the missing actual observation rather than a forged summary.
    invalid["settlements"][0]["proof"]["observations"] = json!([null, null, null]);
    assert_eq!(invalid["settlements"][0]["physical"], json!("pending"));
    assert_eq!(invalid["settlements"][0]["evidence"], json!("pending"));
    assert_eq!(invalid["lifetimes"][0]["state"], json!("closing"));
    assert_eq!(invalid["close_completions"], json!([]));
    reject_body_without_rewrite(
        &path,
        &serde_json::to_string(&invalid).unwrap(),
        "resource proof requires an actual observation even with a matching Pending summary",
    )
    .await;
}

#[tokio::test]
async fn restored_completion_cannot_borrow_authority_from_a_nonresource_observation() {
    let directory = private_directory();
    let path = directory
        .path()
        .join("private/nonresource-completion.sqlite3");
    let mut graph = closing_resource();
    let _ = resource_report(
        &mut graph,
        PhysicalFact::Released,
        EvidenceFact::Acknowledged,
    );
    complete(&mut graph, "root");
    let restored = reopen_snapshot(&path, &graph.snapshot()).await;
    assert_eq!(
        restored.lifetime_state(&lifetime("root")),
        Some(LifetimeState::Closed)
    );
    let mut invalid: Value = serde_json::from_str(&stored_body(&path)).unwrap();
    invalid["settlements"][0]["proof"]["observations"][2]["record"]["detail"] =
        json!({"kind": "Completion"});
    reject_body_without_rewrite(
        &path,
        &serde_json::to_string(&invalid).unwrap(),
        "Completion detail cannot act as a resource observation",
    )
    .await;
}
