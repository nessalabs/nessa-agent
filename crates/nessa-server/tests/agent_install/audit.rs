use super::*;
use crate::agent_install::domain::{
    InstallAttempt, InstallTransitionKind, RollbackState, RuntimeArtifact,
};
use crate::agent_install_test_support::{
    agent, platform, release, request, temporary_root, PINNED_DIGEST,
};
use nessa_auth::application::ports::Clock;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

struct FixedClock;

impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        42
    }
}

fn target() -> RuntimeArtifact {
    RuntimeArtifact::for_release(&release("1.18.31", PINNED_DIGEST, &platform()))
}

#[test]
fn a_record_is_private_durable_json_with_adapter_owned_identity_and_time() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    audit.record(started).unwrap();

    let entries: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    assert_eq!(entries.len(), 1);
    let entry = entries[0].as_ref().unwrap();
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
    assert_eq!(record["observedAtMs"], 42);
    assert_eq!(record["sequence"], 1);
    assert!(record["recordId"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(record["initiator"]["accountId"], "unix:501");
    assert_eq!(record["correlationId"], "install-request-1");
}

#[test]
fn replacement_keeps_the_exact_previous_and_new_artifacts() {
    let previous = RuntimeArtifact::for_release(&release(
        "1.17.0",
        "0000000000000000000000000000000000000000000000000000000000000000",
        &platform(),
    ));
    let (mut attempt, _) = InstallAttempt::start(agent(), target(), request());
    attempt.verified().unwrap();
    let replaced = attempt.replaced(previous).unwrap();
    let value = record_value(&replaced);

    assert_eq!(value["kind"], "agent_runtime_replaced");
    assert_eq!(
        value["transition"]["before"]["artifact"]["version"],
        "1.17.0"
    );
    assert_eq!(
        value["transition"]["after"]["artifact"]["version"],
        "1.18.31"
    );
}

#[test]
fn rollback_says_when_no_prior_runtime_was_restored() {
    let (mut attempt, _) = InstallAttempt::start(agent(), target(), request());
    attempt.verified().unwrap();
    let rolled_back = attempt
        .rolled_back(RollbackState::NoInstalledRuntime)
        .unwrap();
    assert_eq!(rolled_back.kind(), InstallTransitionKind::RolledBack);
    let value = record_value(&rolled_back);
    assert_eq!(value["transition"]["after"]["state"], "not_installed");
    assert_eq!(value["cause"], "publication_failed");
}

#[test]
fn a_durability_failure_is_reported_to_the_use_case() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit"),
        Arc::new(FixedClock),
    )
    .unwrap();
    std::fs::remove_file(directory.join("audit.lock")).unwrap();
    std::fs::remove_dir(&directory).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit.record(started).unwrap_err();

    assert!(!failure.0.is_empty());
    assert!(!directory.exists());
}

struct SequenceClock(Mutex<VecDeque<u64>>);

impl Clock for SequenceClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.lock().unwrap().pop_front().unwrap()
    }
}

#[test]
fn durable_sequence_orders_records_when_the_clock_moves_backwards() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let clock = SequenceClock(Mutex::new(VecDeque::from([100, 50])));
    let audit =
        DurableInstallAudit::new(root.path(), std::path::Path::new("audit"), Arc::new(clock))
            .unwrap();
    let (mut attempt, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    audit.record(attempt.verified().unwrap()).unwrap();

    let first: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("00000000000000000001.json")).unwrap(),
    )
    .unwrap();
    let second: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("00000000000000000002.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (first["sequence"].as_u64(), second["sequence"].as_u64()),
        (Some(1), Some(2))
    );
    assert_eq!(
        (
            first["observedAtMs"].as_u64(),
            second["observedAtMs"].as_u64()
        ),
        (Some(100), Some(50))
    );
}

#[test]
fn a_corrupt_tail_prevents_a_later_record_from_being_acknowledged() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let audit = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit"),
        Arc::new(FixedClock),
    )
    .unwrap();
    std::fs::write(directory.join("00000000000000000001.json"), b"{").unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit.record(started).unwrap_err();

    assert!(failure.to_string().contains("invalid audit record"));
    assert!(!directory.join("00000000000000000002.json").exists());
}

#[test]
fn concurrent_audit_instances_allocate_distinct_durable_sequences() {
    let root = temporary_root();
    let directory = root.path().join("audit");
    let first = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let second = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let barrier = std::sync::Barrier::new(3);
    let (_, first_transition) = InstallAttempt::start(agent(), target(), request());
    let (_, second_transition) = InstallAttempt::start(agent(), target(), request());

    std::thread::scope(|threads| {
        threads.spawn(|| {
            barrier.wait();
            first.record(first_transition).unwrap();
        });
        threads.spawn(|| {
            barrier.wait();
            second.record(second_transition).unwrap();
        });
        barrier.wait();
    });

    assert!(directory.join("00000000000000000001.json").is_file());
    assert!(directory.join("00000000000000000002.json").is_file());
}

#[test]
fn nested_audit_directory_is_created_beneath_the_trusted_root() {
    let root = temporary_root();
    let audit = DurableInstallAudit::new(
        root.path(),
        std::path::Path::new("audit/agent-install"),
        Arc::new(FixedClock),
    )
    .unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());
    audit.record(started).unwrap();
    assert!(root
        .path()
        .join("audit/agent-install/00000000000000000001.json")
        .is_file());
}
