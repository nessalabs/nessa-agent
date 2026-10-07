use super::*;
use std::sync::atomic::Ordering;
#[test]
fn authorization_audit_keeps_current_thread_heartbeat_running() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audit.jsonl");
    let mut audit = FileAuthorizationAudit::new(&path);
    super::super::physical_tests::heartbeat(|gate| async move {
        Arc::get_mut(&mut audit.state).unwrap().gate = Some(gate);
        audit
            .record(&AuthAuditRecord {
                server: uuid::Uuid::new_v4(),
                action: "authorize",
                intent: true,
                generation: 1,
                resource: "https://mcp.example/mcp".into(),
                phase: "consent_needed".into(),
            })
            .await
            .unwrap();
    });
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["action"], "authorize");
}
fn record() -> AuthAuditRecord {
    AuthAuditRecord {
        server: uuid::Uuid::new_v4(),
        action: "authorize",
        intent: true,
        generation: 1,
        resource: "https://mcp.example/mcp".into(),
        phase: "consent_needed".into(),
    }
}

#[tokio::test]
async fn canceled_audit_excludes_next_append() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audit.jsonl");
    let audit = Arc::new(FileAuthorizationAudit::new(&path));
    let probe = audit.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let first = audit.clone();
    let caller = tokio::spawn(async move { first.record(&record()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    let _ = caller.await;
    let mut next = record();
    next.generation = 2;
    audit.record(&next).await.unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    let bytes = std::fs::read_to_string(path).unwrap();
    let lines: Vec<serde_json::Value> = bytes
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["generation"], 1);
    assert_eq!(lines[1]["generation"], 2);
}

#[tokio::test]
async fn worker_input_drop_keeps_slot() {
    let directory = tempfile::tempdir().unwrap();
    let audit = Arc::new(FileAuthorizationAudit::new(
        directory.path().join("audit.jsonl"),
    ));
    let probe = audit.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(gate);
    let first = audit.clone();
    let caller = tokio::spawn(async move { first.record(&record()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    let _ = caller.await;
    audit.record(&record()).await.unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    assert_eq!(probe.inputs_at_release.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn interrupted_audit_and_poison_are_failures() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audit.jsonl");
    let audit = FileAuthorizationAudit::new(&path);
    audit.probe.panic_before.store(true, Ordering::SeqCst);
    assert_eq!(audit.record(&record()).await, Err(AuditFailure));
    assert!(!path.exists());
    audit.probe.panic_after.store(true, Ordering::SeqCst);
    assert_eq!(audit.record(&record()).await, Err(AuditFailure));
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
    let state = audit.state.clone();
    assert!(std::thread::spawn(move || {
        let _guard = state.lock.lock().unwrap();
        panic!("poison audit mutex");
    })
    .join()
    .is_err());
    assert_eq!(audit.record(&record()).await, Err(AuditFailure));
}

#[test]
fn no_runtime_first_poll_is_typed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audit.jsonl");
    let audit = FileAuthorizationAudit::new(&path);
    assert_eq!(
        super::super::physical_tests::plain_thread(audit.record(&record())),
        Err(AuditFailure)
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn ready_effect_input_drop_fault_is_uncertain() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audit.jsonl");
    let audit = FileAuthorizationAudit::new(&path);
    audit.probe.panic_input_drop.store(true, Ordering::SeqCst);
    assert_eq!(audit.record(&record()).await, Err(AuditFailure));
    let lines = std::fs::read_to_string(&path).unwrap();
    assert_eq!(lines.lines().count(), 1);
    assert!(serde_json::from_str::<serde_json::Value>(lines.lines().next().unwrap()).is_ok());
}

#[tokio::test]
async fn operation_panic_input_drop_keeps_slot() {
    let directory = tempfile::tempdir().unwrap();
    let audit = Arc::new(FileAuthorizationAudit::new(
        directory.path().join("audit.jsonl"),
    ));
    let probe = audit.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(gate);
    probe.panic_after.store(true, Ordering::SeqCst);
    let first = audit.clone();
    let caller = tokio::spawn(async move { first.record(&record()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    let _ = caller.await;
    audit.record(&record()).await.unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
}
