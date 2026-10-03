//! Storage-boundary fixtures use exported immutable values and actual producer receipts.

use super::opened;
use nessa_sdk::{
    application::agent_execution::{
        executions::{ExecutionRequest, SubmissionMode},
        permissions::ActionContext,
        sessions::{
            InvocationRecord, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
            SubmissionAcknowledgement,
        },
    },
    domain::agent_execution::{
        executions::ExecutionId,
        prompts::{PromptText, UserMessage},
        sessions::SessionId,
    },
    infrastructure::session_storage::RecordStorage,
};
use nessa_sync::replication::{
    application::RecordSource,
    domain::{Id, PageRequest},
    infrastructure::MAX_PAGE_PAYLOAD,
};
use rusqlite::Connection;
use std::path::Path;

pub(super) fn input_record(name: &str, bytes: usize) -> InvocationRecord {
    InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: ExecutionId::new(name).unwrap(),
            user_message: UserMessage::text_only(PromptText::new("x".repeat(bytes)).unwrap()),
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("user", "phone", "send").unwrap(),
        acknowledgement: SubmissionAcknowledgement::Pending,
        events: Vec::new(),
        scheduling: Vec::new(),
        cancellation: None,
        provider_report: None,
        local_cancellation: None,
        local_outcome: None,
        result: None,
    }
}
pub(super) fn partial_input(bytes: usize) -> SessionChange {
    SessionChange::InputAccepted(Box::new(input_record("partial", bytes)))
}
pub(super) fn with_input(prior: &SessionSnapshot, change: &SessionChange) -> SessionSnapshot {
    let SessionChange::InputAccepted(record) = change else {
        panic!("fixture accepts one input")
    };
    SessionSnapshot {
        invocations: prior
            .invocations
            .iter()
            .cloned()
            .chain(std::iter::once(record.as_ref().clone()))
            .collect(),
        ..prior.clone()
    }
}
pub(super) fn sql(root: &Path, statement: &str) {
    Connection::open(root.join("records.sqlite3"))
        .unwrap()
        .execute_batch(statement)
        .unwrap();
}
pub(super) fn rows(root: &Path) -> i64 {
    Connection::open(root.join("records.sqlite3"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM event_records", [], |row| row.get(0))
        .unwrap()
}
pub(super) fn refuse_offset(root: &Path, trigger: &str, offset: u64) {
    sql(root, &format!(
        "CREATE TRIGGER {trigger} BEFORE INSERT ON event_records WHEN NEW.offset=X'{offset:016X}' BEGIN SELECT RAISE(ABORT, 'fixture physical append refusal'); END;"
    ));
}
pub(super) async fn opening_completion_offset(id: &SessionId) -> u64 {
    let directory = tempfile::tempdir().unwrap();
    let storage = RecordStorage::new(directory.path().join("records")).unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (change, snapshot) = opened(id);
    let receipt = lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    let source = storage
        .record_source(id, Id::new("origin").unwrap())
        .await
        .unwrap()
        .unwrap();
    let terminal = receipt.next().base();
    tokio::task::spawn_blocking(move || {
        let mut source = source;
        let scope = source.scope(Id::new("receiver").unwrap(), Id::new("epoch").unwrap());
        assert_eq!(source.head(&scope).unwrap(), terminal);
        let page = source
            .page(&PageRequest {
                scope,
                after: 0,
                target: terminal,
                max_records: 64,
                max_payload_bytes: MAX_PAGE_PAYLOAD,
                max_record_bytes: MAX_PAGE_PAYLOAD,
            })
            .unwrap();
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.records.last().unwrap().position, terminal);
        assert_eq!(page.records.first().unwrap().position, terminal - 1);
    })
    .await
    .unwrap();
    drop(lease);
    storage.shutdown().await.unwrap();
    terminal
}
