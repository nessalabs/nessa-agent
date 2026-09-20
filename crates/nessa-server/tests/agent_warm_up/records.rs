//! The record is what stops the next process paying for a scan already done.
use super::{FileWarmUpRecords, RuntimeFingerprint, WarmUpRecords};
use std::io::Write;

fn runtime(directory: &str) -> RuntimeFingerprint {
    RuntimeFingerprint::new(
        &format!("/runtimes/{directory}/node"),
        &format!("/runtimes/{directory}/acp/index.js"),
        "claude-sonnet-5",
    )
    .unwrap()
}

#[tokio::test]
async fn a_completed_warm_up_is_remembered_for_that_runtime_only() {
    let root = tempfile::tempdir().unwrap();
    let records = FileWarmUpRecords::new(root.path().join("warm-up")).unwrap();
    let installed = runtime("aa");
    let updated = runtime("bb");
    assert!(!records.completed(&installed).await.unwrap());
    records
        .record_completed(installed.clone(), 1_000)
        .await
        .unwrap();
    assert!(records.completed(&installed).await.unwrap());
    // An update is a different runtime and is scanned again, so its warm-up has
    // not been done however many times the previous one was.
    assert!(!records.completed(&updated).await.unwrap());
}

#[tokio::test]
async fn recording_the_same_runtime_twice_states_one_fact() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("warm-up");
    let records = FileWarmUpRecords::new(directory.clone()).unwrap();
    let installed = runtime("aa");
    records
        .record_completed(installed.clone(), 1_000)
        .await
        .unwrap();
    records
        .record_completed(installed.clone(), 2_000)
        .await
        .unwrap();
    assert!(records.completed(&installed).await.unwrap());
    let files: Vec<_> = std::fs::read_dir(&directory).unwrap().collect();
    assert_eq!(files.len(), 1, "one runtime, one record");
}

#[tokio::test]
async fn a_damaged_record_warms_again_instead_of_refusing_to() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("warm-up");
    let records = FileWarmUpRecords::new(directory.clone()).unwrap();
    let installed = runtime("aa");
    records
        .record_completed(installed.clone(), 1_000)
        .await
        .unwrap();
    let path = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"{ truncated").unwrap();
    drop(file);
    // Warming again costs a launch. Refusing to warm because a marker is
    // unreadable would cost the user their first message instead.
    assert!(!records.completed(&installed).await.unwrap());
}

#[tokio::test]
async fn an_oversized_record_is_reported_rather_than_silently_ignored() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("warm-up");
    let records = FileWarmUpRecords::new(directory.clone()).unwrap();
    let installed = runtime("aa");
    records
        .record_completed(installed.clone(), 1_000)
        .await
        .unwrap();
    let path = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&path, vec![b'x'; 16_385]).unwrap();
    assert!(records.completed(&installed).await.is_err());
}
