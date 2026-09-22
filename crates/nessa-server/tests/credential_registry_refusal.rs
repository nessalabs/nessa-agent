//! Process-boundary regressions for issue #113.

use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const INVALID: &[u8] = br#"{"schemaVersion":1,"credentialSecret":"must-not-appear"}"#;

struct Run {
    _root: tempfile::TempDir,
    registry: PathBuf,
    auth: PathBuf,
    output: Output,
}

fn run(args: &[&str], obstruct_audit: bool) -> Run {
    let root = tempfile::tempdir().unwrap();
    let auth = root.path().join("ci/auth");
    nessa_local_storage::create_directory(&auth).unwrap();
    let registry = auth.join("credentials.v1.json");
    let mut file =
        nessa_local_storage::open(&registry, nessa_local_storage::OpenMode::CreateNew).unwrap();
    file.write_all(INVALID).unwrap();
    file.sync_all().unwrap();
    drop(file);
    if obstruct_audit {
        nessa_local_storage::open(
            &auth.join("audit"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args(args)
        .env_clear()
        .env("NESSA_DATA_DIR", root.path())
        .env("NESSA_STAGE", "ci")
        .output()
        .unwrap();
    Run {
        _root: root,
        registry,
        auth,
        output,
    }
}

fn stderr(run: &Run) -> String {
    String::from_utf8_lossy(&run.output.stderr).into_owned()
}

fn records(auth: &Path) -> Vec<Value> {
    let directory = auth.join("audit/credential-registry-refusals");
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
        })
        .collect()
}

fn assert_refusal(run: &Run, expected_cause: &str) {
    let message = stderr(run);
    assert_eq!(run.output.status.code(), Some(28), "{message}");
    assert!(
        message.contains(run.registry.to_str().unwrap()),
        "{message}"
    );
    assert!(
        message.contains("schema 1 is unsupported; expected schema 2"),
        "{message}"
    );
    assert!(message.contains("The file was left unchanged"), "{message}");
    assert!(
        message.contains("Restore a verified backup to the same path"),
        "{message}"
    );
    assert!(
        message.contains("reinitializing creates new identities"),
        "{message}"
    );
    assert!(!message.contains("must-not-appear"), "{message}");
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);

    let records = records(&run.auth);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["kind"], "credential_registry_refused");
    assert_eq!(record["target"]["value"], run.registry.to_str().unwrap());
    assert_eq!(record["transition"]["before"], "registry_present_untrusted");
    assert_eq!(
        record["transition"]["after"],
        "registry_open_refused_file_preserved"
    );
    assert_eq!(record["cause"], expected_cause);
    assert_eq!(record["initiator"]["kind"], "automatic");
    assert_eq!(record["fault"]["kind"], "unsupported_schema");
    assert_eq!(record["fault"]["found"], 1);
    assert_eq!(record["fault"]["expected"], 2);
    assert!(!serde_json::to_string(record)
        .unwrap()
        .contains("must-not-appear"));
}

#[test]
fn ordinary_server_refuses_corruption_with_action_exit_code_and_audit() {
    let run = run(&["server"], false);
    assert_refusal(&run, "gateway_startup");
}

#[test]
fn automatic_provisioning_uses_the_same_audited_refusal_boundary() {
    let run = run(&["server", "--provision-local"], false);
    assert_refusal(&run, "automatic_local_provisioning");
}

#[test]
fn audit_failure_keeps_the_original_refusal_and_registry_bytes_visible() {
    let run = run(&["server"], true);
    let message = stderr(&run);
    assert_eq!(run.output.status.code(), Some(28), "{message}");
    assert!(
        message.contains(run.registry.to_str().unwrap()),
        "{message}"
    );
    assert!(
        message.contains("schema 1 is unsupported; expected schema 2"),
        "{message}"
    );
    assert!(
        message.contains("refusal audit was not recorded"),
        "{message}"
    );
    assert!(!message.contains("must-not-appear"), "{message}");
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);
    assert!(run.auth.join("audit").is_file());
}
