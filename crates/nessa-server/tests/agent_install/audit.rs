use super::*;
use crate::agent_install::domain::{
    InstallAttempt, InstallTransitionKind, RollbackState, RuntimeArtifact,
};
use crate::agent_install_test_support::{agent, platform, release, request, PINNED_DIGEST};
use nessa_auth::application::ports::Clock;
use std::sync::Arc;

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
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("audit");
    let audit = DurableInstallAudit::new(directory.clone(), Arc::new(FixedClock)).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    audit.record(started).unwrap();

    let entries: Vec<_> = std::fs::read_dir(&directory).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let entry = entries[0].as_ref().unwrap();
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
    assert_eq!(record["observedAtMs"], 42);
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
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("audit");
    let audit = DurableInstallAudit::new(directory.clone(), Arc::new(FixedClock)).unwrap();
    std::fs::remove_dir(&directory).unwrap();
    let (_, started) = InstallAttempt::start(agent(), target(), request());

    let failure = audit.record(started).unwrap_err();

    assert!(!failure.0.is_empty());
    assert!(!directory.exists());
}
