//! JSONL saves append only changed records and recover unacknowledged tails.
use super::*;
use std::io::Write;

#[tokio::test]
async fn journal_appends_small_tails_without_rewriting_prior_history() {
    let root = tempfile::tempdir().unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let lease = storage.open(id("incremental")).await.unwrap();
    let mut value = snapshot("incremental");
    value.invocations[0].events.clear();
    value.invocations[0].result = None;
    value.invocations[0].request.user_message = PromptText::new("input ".repeat(1000)).unwrap();
    lease.save(value.clone()).await.unwrap();
    let path = journal_path(&root.path().join("private"), "incremental");
    let first = std::fs::read(&path).unwrap();
    let execution_id = value.invocations[0].request.execution_id.clone();
    for _ in 0..20 {
        value.invocations[0].events.push(ExecutionEvent::new(
            execution_id.clone(),
            ExecutionUpdate::Message(MessageChunk::text("next")),
        ));
        lease.save(value.clone()).await.unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(&first));
    let lines = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 21);
    for (index, line) in lines.iter().enumerate().skip(1) {
        assert!(
            line.len() < 500,
            "an appended event must not repeat prompt/history"
        );
        let change: serde_json::Value = serde_json::from_slice(line).unwrap();
        assert_eq!(change["sequence"], index + 1);
        assert!(change["invocations"][0].get("metadata").is_none());
        assert_eq!(
            change["invocations"][0]["events"].as_array().unwrap().len(),
            1
        );
    }
    lease.save(value.clone()).await.unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "retry must not append duplicates"
    );
    drop(lease);
    let lease = storage.open(id("incremental")).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    // The snapshot API still permits indexed replacements and truncation.
    value.invocations[0].events.truncate(3);
    value.invocations[0].events[1] = ExecutionEvent::new(
        value.invocations[0].request.execution_id.clone(),
        ExecutionUpdate::Message(MessageChunk::text("replacement")),
    );
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    value.invocations.clear();
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}

#[tokio::test]
async fn journal_recovers_only_incomplete_tail_and_rejects_complete_corruption() {
    let root = tempfile::tempdir().unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let lease = storage.open(id("tail")).await.unwrap();
    let value = snapshot("tail");
    lease.save(value.clone()).await.unwrap();
    let path = journal_path(&root.path().join("private"), "tail");
    let committed = std::fs::read(&path).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"sequence\":2,\"id\":\"tail\"")
        .unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
    assert_eq!(std::fs::read(&path).unwrap(), committed);
    lease.save(value.clone()).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), committed);
    for suffix in [b"{broken}\n".as_slice(), committed.as_slice()] {
        let mut corrupt = committed.clone();
        corrupt.extend_from_slice(suffix);
        std::fs::write(&path, &corrupt).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert!(matches!(
            lease.save(value.clone()).await,
            Err(StorageError::Corrupt(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
}

#[tokio::test]
async fn journal_incomplete_first_save_is_uncommitted_and_can_be_retried() {
    let root = tempfile::tempdir().unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let path = journal_path(&root.path().join("private"), "first");
    private::open(&path, private::OpenMode::CreateNew)
        .unwrap()
        .write_all(b"{\"sequence\":1")
        .unwrap();
    let lease = storage.open(id("first")).await.unwrap();
    assert!(lease.load().await.unwrap().is_none());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    let value = snapshot("first");
    lease.save(value.clone()).await.unwrap();
    assert_same(&lease.load().await.unwrap().unwrap(), &value);
}

#[tokio::test]
async fn journal_rejects_invalid_complete_checkpoints_even_when_later_records_replace_them() {
    let root = tempfile::tempdir().unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let lease = storage.open(id("checkpoint")).await.unwrap();
    let mut value = snapshot("checkpoint");
    value.invocations[0].events.clear();
    lease.save(value.clone()).await.unwrap();
    let path = journal_path(&root.path().join("private"), "checkpoint");
    let initial = std::fs::read(&path).unwrap();
    value.provider = ProviderIdentity::new("next", "next", "next").unwrap();
    value.invocations.clear();
    lease.save(value).await.unwrap();
    let complete = std::fs::read(&path).unwrap();
    for field in ["provider", "execution_id", "result"] {
        let mut first: serde_json::Value = serde_json::from_slice(&initial).unwrap();
        match field {
            "provider" => first["provider"]["name"] = "".into(),
            "execution_id" => first["invocations"][0]["metadata"]["execution_id"] = "".into(),
            _ => {
                first["invocations"][0]["events"] = serde_json::json!([
                    {"execution_id": "execution", "update": {"Finished": "Cancelled"}}
                ]);
                first["invocations"][0]["metadata"]["result"] =
                    serde_json::json!({"Ok": "Completed"});
            }
        }
        let mut corrupt = serde_json::to_vec(&first).unwrap();
        corrupt.push(b'\n');
        corrupt.extend_from_slice(&complete[initial.len()..]);
        std::fs::write(&path, &corrupt).unwrap();
        assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    }
}
