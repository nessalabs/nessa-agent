use nessa_vault::{
    application::{Audit, AuditRecord, Caller, ObservationClock},
    domain::{Failure, Status},
    infrastructure::FileAudit,
};
use serde_json::{json, Value};
use std::{io::Write, sync::Arc};
struct Time;
impl ObservationClock for Time {
    fn unix_millis(&self) -> Result<u64, Failure> {
        Ok(10)
    }
}
fn record() -> AuditRecord {
    AuditRecord {
        target: None,
        operation: 3,
        lease: "session".into(),
        caller: Caller {
            principal: "owner".into(),
            credential: "desktop".into(),
        },
        action: "unlock",
        before: Status::Locked,
        after: Status::Locked,
        cause: "explicit_request",
        result: "requested",
    }
}
fn row(sequence: u64, operation: u64) -> Value {
    json!({"target":null,"sequence":sequence,"operation":operation,"observedAtUnixMs":20,
        "lease":"session","principal":"owner","credential":"desktop",
        "action":"unlock","before":"locked","after":"locked","cause":"explicit_request","result":"requested"})
}
fn seeded(rows: &[Value]) -> (tempfile::TempDir, FileAudit, Vec<u8>) {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("audit");
    nessa_local_storage::create_directory(&directory).unwrap();
    let bytes = serde_json::to_vec(rows).unwrap();
    nessa_local_storage::open(
        &directory.join("events.json"),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap()
    .write_all(&bytes)
    .unwrap();
    let audit = FileAudit::new(directory, Arc::new(Time));
    (root, audit, bytes)
}
#[test]
fn malformed_retained_evidence_is_refused_without_replacing_original_bytes() {
    let mut variants = Vec::new();
    for (field, value) in [
        ("before", json!("invented")),
        ("action", json!("invented")),
        ("result", json!("published")),
        ("cause", json!("encrypted_receipt")),
        ("after", json!("unlocked")),
        ("target", json!("../escape")),
        ("observedAtUnixMs", json!(9_007_199_254_740_992u64)),
        ("credential", json!("x".repeat(257))),
    ] {
        let mut invalid = row(7, 0);
        invalid[field] = value;
        variants.push(vec![invalid]);
    }
    variants.push(vec![row(7, 0), row(7, 1)]);
    variants.push(vec![row(7, 0), row(9, 1)]);
    variants.push(vec![row(7, 0), row(8, 0)]);
    let mut conflicting_actor = row(8, 1);
    conflicting_actor["principal"] = json!("another-owner");
    variants.push(vec![row(7, 0), conflicting_actor]);
    for rows in variants {
        let (root, audit, original) = seeded(&rows);
        assert_eq!(audit.record(&record()), Err(Failure::AuditUnavailable));
        assert_eq!(
            std::fs::read(root.path().join("audit/events.json")).unwrap(),
            original
        );
    }
}
#[test]
fn a_valid_retained_suffix_and_reversing_observation_clock_remain_accepted() {
    let (root, audit, _) = seeded(&[row(7, 0), row(8, 1)]);
    audit.record(&record()).unwrap();
    let rows: Vec<Value> =
        serde_json::from_slice(&std::fs::read(root.path().join("audit/events.json")).unwrap())
            .unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["sequence"], 9);
    assert_eq!(rows[2]["observedAtUnixMs"], 10);
}
