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
    run_with(
        |_| args.iter().map(|value| (*value).to_owned()).collect(),
        |auth, _| {
            if obstruct_audit {
                nessa_local_storage::open(
                    &auth.join("audit"),
                    nessa_local_storage::OpenMode::CreateNew,
                )
                .unwrap();
            }
        },
    )
}

fn run_with(args: impl FnOnce(&Path) -> Vec<String>, setup: impl FnOnce(&Path, &Path)) -> Run {
    let root = tempfile::tempdir().unwrap();
    let auth = root.path().join("ci/auth");
    nessa_local_storage::create_directory(&auth).unwrap();
    let registry = auth.join("credentials.v1.json");
    let mut file =
        nessa_local_storage::open(&registry, nessa_local_storage::OpenMode::CreateNew).unwrap();
    file.write_all(INVALID).unwrap();
    file.sync_all().unwrap();
    drop(file);
    setup(&auth, &registry);
    let args = args(root.path());
    let output = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args(&args)
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

#[test]
fn explicit_offline_command_records_only_its_verified_process_attribution() {
    let run = run_with(
        |root| {
            vec![
                "auth".into(),
                "recover-owner".into(),
                "--local".into(),
                "--owner-token-file".into(),
                root.join("new-owner.token").to_str().unwrap().to_owned(),
            ]
        },
        |_, _| {},
    );

    assert_eq!(run.output.status.code(), Some(28), "{}", stderr(&run));
    let records = records(&run.auth);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["cause"], "local_auth_command");
    assert_eq!(
        records[0]["initiator"]["kind"],
        "local_process_unattributed"
    );
}

#[cfg(unix)]
fn assert_unsafe_refusal(run: &Run, target: &Path, role: &str) {
    let message = stderr(run);
    assert_eq!(run.output.status.code(), Some(28), "{message}");
    assert!(
        message.contains("not private, single-linked storage"),
        "{message}"
    );
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);
    let records = records(&run.auth);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["target"]["value"], target.to_str().unwrap());
    assert_eq!(records[0]["fault"]["kind"], "unsafe_storage");
    assert_eq!(records[0]["fault"]["role"], role);
}

#[cfg(unix)]
#[test]
fn public_registry_permissions_are_refused_and_audited_without_rewrite() {
    use std::os::unix::fs::PermissionsExt;

    let run = run_with(
        |_| vec!["server".into()],
        |_, registry| {
            fs::set_permissions(registry, fs::Permissions::from_mode(0o644)).unwrap();
        },
    );

    assert_unsafe_refusal(&run, &run.registry, "registry");
    assert_eq!(
        fs::metadata(&run.registry).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[cfg(unix)]
#[test]
fn hard_linked_registry_is_refused_and_audited_without_unlinking_evidence() {
    let run = run_with(
        |_| vec!["server".into()],
        |auth, registry| fs::hard_link(registry, auth.join("registry-evidence-link")).unwrap(),
    );

    assert_unsafe_refusal(&run, &run.registry, "registry");
    assert_eq!(
        fs::read(run.auth.join("registry-evidence-link")).unwrap(),
        INVALID
    );
}

#[cfg(unix)]
#[test]
fn registry_symlink_is_refused_and_audited_without_changing_its_target() {
    use std::os::unix::fs::symlink;

    let run = run_with(
        |_| vec!["server".into()],
        |auth, registry| {
            let evidence = auth.join("registry-evidence.json");
            fs::rename(registry, &evidence).unwrap();
            symlink(&evidence, registry).unwrap();
        },
    );

    assert_unsafe_refusal(&run, &run.registry, "registry");
    assert_eq!(
        fs::read(run.auth.join("registry-evidence.json")).unwrap(),
        INVALID
    );
}

#[cfg(unix)]
fn lock_path(auth: &Path) -> PathBuf {
    auth.join("credentials.v1.lock")
}

#[cfg(unix)]
#[test]
fn public_lock_permissions_are_refused_and_audited_without_touching_the_registry() {
    use std::os::unix::fs::PermissionsExt;

    let run = run_with(
        |_| vec!["server".into()],
        |auth, _| {
            let lock = lock_path(auth);
            nessa_local_storage::open(&lock, nessa_local_storage::OpenMode::CreateNew).unwrap();
            fs::set_permissions(lock, fs::Permissions::from_mode(0o644)).unwrap();
        },
    );
    let lock = lock_path(&run.auth);

    assert_unsafe_refusal(&run, &lock, "lock");
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);
    assert_eq!(
        fs::metadata(lock).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(!stderr(&run).contains("auth init"));
}

#[cfg(unix)]
#[test]
fn hard_linked_lock_is_refused_and_audited_without_unlinking_evidence() {
    let run = run_with(
        |_| vec!["server".into()],
        |auth, _| {
            let lock = lock_path(auth);
            nessa_local_storage::open(&lock, nessa_local_storage::OpenMode::CreateNew).unwrap();
            fs::hard_link(&lock, auth.join("lock-evidence-link")).unwrap();
        },
    );
    let lock = lock_path(&run.auth);

    assert_unsafe_refusal(&run, &lock, "lock");
    assert!(run.auth.join("lock-evidence-link").is_file());
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);
}

#[cfg(unix)]
#[test]
fn lock_symlink_is_refused_and_audited_without_changing_its_target() {
    use std::os::unix::fs::symlink;

    let run = run_with(
        |_| vec!["server".into()],
        |auth, _| {
            let evidence = auth.join("lock-evidence");
            nessa_local_storage::open(&evidence, nessa_local_storage::OpenMode::CreateNew).unwrap();
            symlink(&evidence, lock_path(auth)).unwrap();
        },
    );
    let lock = lock_path(&run.auth);

    assert_unsafe_refusal(&run, &lock, "lock");
    assert!(run.auth.join("lock-evidence").is_file());
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);
}

#[cfg(unix)]
#[test]
fn audit_ancestry_symlink_reports_sink_failure_without_writing_outside_root() {
    use std::os::unix::fs::symlink;

    let run = run_with(
        |_| vec!["server".into()],
        |auth, _| {
            let outside = auth.parent().unwrap().join("outside-audit");
            nessa_local_storage::create_directory(&outside).unwrap();
            symlink(&outside, auth.join("audit")).unwrap();
        },
    );

    let message = stderr(&run);
    assert_eq!(run.output.status.code(), Some(28), "{message}");
    assert!(
        message.contains("refusal audit was not recorded"),
        "{message}"
    );
    assert_eq!(fs::read(&run.registry).unwrap(), INVALID);
    assert_eq!(
        fs::read_dir(run.auth.parent().unwrap().join("outside-audit"))
            .unwrap()
            .count(),
        0
    );
}
