//! Public ownership adapter refuses a persisted nonresource slot without rewriting evidence.
use nessa_sdk::{
    application::agent_execution::subagents::{OwnershipStore, PortFailure},
    domain::agent_execution::{
        sessions::SessionId,
        subagents::{
            AgentLifetimeId, ApprovalPolicy, CloseOperationId, EvidenceFact, HostActor, Initiator,
            LifetimeCause, LifetimeState, OwnershipGraph, PhysicalFact, SpawnAdmission,
            SpawnBinding, SpawnOrigin, SpawnRequestId, TaskDigest,
        },
    },
    infrastructure::session_storage::SqliteOwnershipStore,
};
use serde_json::{json, Value};

#[tokio::test]
async fn public_sqlite_restore_refuses_nonresource_observation_with_actual_completion_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("owned.sqlite3");
    let root = AgentLifetimeId::new("root").unwrap();
    let operation = CloseOperationId::new("close").unwrap();
    let mut graph = OwnershipGraph::new();
    let _ = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            root.clone(),
            Initiator::Runtime,
        )
        .unwrap();
    let _ = graph
        .begin_close(
            &root,
            operation.clone(),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    let observation = graph
        .apply_report(
            &root,
            &operation,
            &root,
            PhysicalFact::Released,
            EvidenceFact::Acknowledged,
        )
        .unwrap();
    graph
        .acknowledge_observation(&observation, EvidenceFact::Acknowledged)
        .unwrap();
    let completion = graph.prepare_completion(&root, &operation).unwrap();
    graph
        .acknowledge_completion(&completion, EvidenceFact::Acknowledged)
        .unwrap();
    let snapshot = graph.snapshot();
    let store = SqliteOwnershipStore::open(&path).unwrap();
    store.write(&snapshot).await.unwrap();
    assert_eq!(store.read().await.unwrap(), snapshot);
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let body: String = connection
        .query_row(
            "SELECT body FROM ownership_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut invalid: Value = serde_json::from_str(&body).unwrap();
    invalid["settlements"][0]["proof"]["observations"][2]["record"]["detail"] =
        json!({"kind": "Completion"});
    let invalid = serde_json::to_string(&invalid).unwrap();
    connection
        .execute(
            "UPDATE ownership_snapshot SET body = ?1 WHERE id = 1",
            [&invalid],
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&path).unwrap();
    let store = SqliteOwnershipStore::open(&path).unwrap();
    assert_eq!(store.read().await, Err(PortFailure::Rejected));
    drop(store);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let retained: String = connection
        .query_row(
            "SELECT body FROM ownership_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, invalid);
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn public_sqlite_refuses_a_partly_closed_completed_cascade_without_rewrite() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("private");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("group.sqlite3");
    let mut graph = OwnershipGraph::new();
    let root = AgentLifetimeId::new("a").unwrap();
    let operation = CloseOperationId::new("close").unwrap();
    let _ = graph
        .open_root(
            SessionId::new("a").unwrap(),
            root.clone(),
            Initiator::Runtime,
        )
        .unwrap();
    for (parent, child) in [("a", "middle"), ("middle", "z")] {
        let _ = graph
            .admit_spawn(SpawnAdmission {
                child_lifetime: AgentLifetimeId::new(child).unwrap(),
                child_session: SessionId::new(child).unwrap(),
                binding: SpawnBinding {
                    parent_lifetime: AgentLifetimeId::new(parent).unwrap(),
                    parent_session: SessionId::new(parent).unwrap(),
                    request_id: SpawnRequestId::new(child).unwrap(),
                    task_digest: TaskDigest::new("a".repeat(64)).unwrap(),
                    policy: ApprovalPolicy::new("read-only", "ask", "revision").unwrap(),
                    model: None,
                    origin: SpawnOrigin::Host(HostActor::new("person", "desktop", child).unwrap()),
                },
                live_room: true,
            })
            .unwrap();
    }
    let _ = graph
        .begin_close(
            &root,
            operation.clone(),
            LifetimeCause::HostClose,
            Initiator::Runtime,
        )
        .unwrap();
    for target in ["a", "middle", "z"] {
        let record = graph
            .apply_report(
                &root,
                &operation,
                &AgentLifetimeId::new(target).unwrap(),
                PhysicalFact::Released,
                EvidenceFact::Acknowledged,
            )
            .unwrap();
        graph
            .acknowledge_observation(&record, EvidenceFact::Acknowledged)
            .unwrap();
    }
    let completion = graph.prepare_completion(&root, &operation).unwrap();
    graph
        .acknowledge_completion(&completion, EvidenceFact::Acknowledged)
        .unwrap();
    let snapshot = graph.snapshot();
    assert!(snapshot
        .lifetimes
        .iter()
        .all(|row| row.state == LifetimeState::Closed));
    for target in ["middle", "z"] {
        let store = SqliteOwnershipStore::open(&path).unwrap();
        store.write(&snapshot).await.unwrap();
        assert_eq!(store.read().await.unwrap(), snapshot);
        drop(store);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let body: String = connection
            .query_row(
                "SELECT body FROM ownership_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut invalid: Value = serde_json::from_str(&body).unwrap();
        let lifetime = invalid["lifetimes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["lifetime_id"] == target)
            .unwrap();
        assert_eq!(lifetime["state"], "closed");
        lifetime["state"] = json!("closing");
        let invalid = serde_json::to_string(&invalid).unwrap();
        connection
            .execute(
                "UPDATE ownership_snapshot SET body = ?1 WHERE id = 1",
                [&invalid],
            )
            .unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        let store = SqliteOwnershipStore::open(&path).unwrap();
        assert_eq!(store.read().await, Err(PortFailure::Rejected));
        drop(store);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let retained: String = connection
            .query_row(
                "SELECT body FROM ownership_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, invalid);
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
}
