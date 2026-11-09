use super::*;
use crate::conversation::{
    application::{background_tasks::BackgroundSource, ConversationCaller},
    domain::{BackgroundCommand, TaskCause, TaskDelivery, TaskLifecycle, TaskOutcome, TaskStop},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
fn actor() -> TaskInitiator {
    TaskInitiator::new("person".into(), "panel".into(), "delete".into()).unwrap()
}
fn record(id: &str) -> BackgroundRecord {
    BackgroundRecord {
        id: BackgroundTaskId::new(id).unwrap(),
        command: BackgroundCommand::new("echo payload", 120).unwrap(),
        source: BackgroundSource {
            conversation: ConversationId::new("00000000-0000-0000-0000-000000001201").unwrap(),
            execution: "original-turn".into(),
            tool_call: format!("native-{id}"),
            caller: ConversationCaller {
                organization_id: OrganizationId::new("org").unwrap(),
                principal_id: PrincipalId::new("person").unwrap(),
                surface_id: "panel".into(),
                action_id: "start".into(),
            },
            workspace: "/tmp".into(),
        },
        lifecycle: TaskLifecycle::admitted(100),
    }
}
fn outcome() -> TaskOutcome {
    TaskOutcome {
        cause: TaskCause::Exited,
        scope: Some(1),
        process: Some(2),
        os_pid: Some(3),
        exit_code: Some(0),
        signal: None,
        cleanup_verified: true,
        stdout: "PRIVATE_OUTPUT".into(),
        stderr: String::new(),
        omitted_bytes: 7,
        output_complete: true,
        error: None,
    }
}
#[tokio::test]
async fn background_store_commits_audit_with_current_state_and_preserves_first_binding() {
    let directory = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&directory.path().join("data")).unwrap();
    let path = directory.path().join("data/tasks.sqlite3");
    let store = LocalBackgroundTasks::open(&path).unwrap();
    let first = record("build");
    store.save(first.clone(), None).await.unwrap();
    store.save(first.clone(), None).await.unwrap();
    let stopped = BackgroundRecord {
        lifecycle: first
            .lifecycle
            .request_stop(TaskStop::ConversationDeleted(actor()))
            .unwrap(),
        ..first.clone()
    };
    store.save(stopped.clone(), Some(0)).await.unwrap();
    let terminal = BackgroundRecord {
        lifecycle: stopped.lifecycle.settle(outcome(), 200).unwrap(),
        ..stopped.clone()
    };
    let mut stolen = terminal.clone();
    stolen.source.tool_call = "retry-row".into();
    assert_eq!(
        store.save(stolen, Some(1)).await,
        Err(BackgroundTaskError::Conflict)
    );
    store.save(terminal.clone(), Some(1)).await.unwrap();
    let delivered = BackgroundRecord {
        lifecycle: terminal
            .lifecycle
            .finish_delivery(TaskDelivery::Delivered)
            .unwrap(),
        ..terminal.clone()
    };
    store.save(delivered.clone(), Some(2)).await.unwrap();
    drop(store);
    let reopened = LocalBackgroundTasks::open(&path).unwrap();
    assert_eq!(
        reopened
            .read(&first.source.conversation, &first.id)
            .await
            .unwrap(),
        Some(delivered.clone())
    );
    assert!(reopened.pending().await.unwrap().is_empty());
    let audit: Vec<String> = reopened
        .connection
        .lock()
        .unwrap()
        .prepare("SELECT evidence FROM transitions ORDER BY revision")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(audit.len(), 4);
    assert!(audit.iter().all(|s| !s.contains("PRIVATE_OUTPUT")));
    let settled: serde_json::Value = serde_json::from_str(&audit[2]).unwrap();
    assert_eq!(settled["process"]["scope"], 1);
    assert_eq!(settled["process"]["process"], 2);
    assert_eq!(settled["process"]["exitCode"], 0);
    assert!(audit[1].contains("conversation_deleted") && audit[1].contains("delete"));
    reopened
        .forget_conversation(&first.source.conversation, actor())
        .await
        .unwrap();
    assert!(reopened
        .read(&first.source.conversation, &first.id)
        .await
        .unwrap()
        .is_none());
    let retained_admission: String = reopened
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT evidence FROM transitions WHERE revision = 0",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let admitted: serde_json::Value = serde_json::from_str(&retained_admission).unwrap();
    assert_eq!(
        admitted["admission"],
        serde_json::json!({
            "command":"echo payload", "timeoutSeconds":120, "workspace":"/tmp", "execution":"original-turn",
            "toolCall":"native-build", "organization":"org", "principal":"person", "surface":"panel", "action":"start"
        })
    );
    assert_eq!(
        reopened
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM transitions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        5
    );
}
#[tokio::test]
async fn background_store_capacity_and_failed_audit_never_partially_admit() {
    let directory = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&directory.path().join("data")).unwrap();
    let store = LocalBackgroundTasks::open(&directory.path().join("data/tasks.sqlite3")).unwrap();
    for index in 0..MAX_CONVERSATION_BACKGROUND_TASKS {
        store
            .save(record(&format!("task-{index}")), None)
            .await
            .unwrap();
    }
    assert_eq!(
        store.save(record("overflow"), None).await,
        Err(BackgroundTaskError::Capacity)
    );
    let first = record("task-0");
    assert_eq!(
        store
            .forget_conversation(&first.source.conversation, actor())
            .await,
        Err(BackgroundTaskError::CleanupUnverified)
    );
    store.connection.lock().unwrap().execute_batch("CREATE TRIGGER refuse_audit BEFORE INSERT ON transitions BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;").unwrap();
    let next = BackgroundRecord {
        lifecycle: first
            .lifecycle
            .request_stop(TaskStop::GatewayStopped(actor()))
            .unwrap(),
        ..first.clone()
    };
    assert_eq!(
        store.save(next, Some(0)).await,
        Err(BackgroundTaskError::Storage)
    );
    assert_eq!(
        store
            .read(&first.source.conversation, &first.id)
            .await
            .unwrap(),
        Some(first)
    );
}

#[tokio::test]
async fn background_store_rejects_index_and_admission_disagreement_before_recovery() {
    for field in [
        "conversation",
        "task",
        "execution",
        "tool_call",
        "revision",
        "command",
        "retained",
    ] {
        let directory = tempfile::tempdir().unwrap();
        nessa_local_storage::create_directory(&directory.path().join("data")).unwrap();
        let path = directory.path().join("data/tasks.sqlite3");
        let store = LocalBackgroundTasks::open(&path).unwrap();
        let first = record("first");
        store.save(first.clone(), None).await.unwrap();
        {
            let connection = store.connection.lock().unwrap();
            if field == "retained" {
                connection
                    .execute("UPDATE tasks SET retained = 0", [])
                    .unwrap();
            } else {
                let mut forged: serde_json::Value =
                    serde_json::from_str(&encoding::encode(&first).unwrap()).unwrap();
                forged[field] = match field {
                    "conversation" => serde_json::json!("00000000-0000-0000-0000-000000001202"),
                    "revision" => serde_json::json!(1),
                    _ => serde_json::json!("other"),
                };
                connection
                    .execute("UPDATE tasks SET record = ?1", [forged.to_string()])
                    .unwrap();
            }
        }
        assert_eq!(
            store.read(&first.source.conversation, &first.id).await,
            Err(BackgroundTaskError::Storage),
            "{field}"
        );
        assert_eq!(
            store
                .for_calls(
                    &first.source.conversation,
                    vec![(
                        first.source.execution.clone(),
                        first.source.tool_call.clone()
                    )]
                )
                .await,
            Err(BackgroundTaskError::Storage),
            "{field}"
        );
        drop(store);
        assert!(
            matches!(
                LocalBackgroundTasks::open(&path),
                Err(OpenError::Damaged(_))
            ),
            "{field}"
        );
    }
}
