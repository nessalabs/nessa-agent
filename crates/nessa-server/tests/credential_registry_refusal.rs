//! Process-boundary regressions for issue #113.

use serde_json::Value;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
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
    let output = execute(root.path(), &args);
    Run {
        _root: root,
        registry,
        auth,
        output,
    }
}

fn execute(root: &Path, args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args(args)
        .env_clear()
        .env("NESSA_DATA_DIR", root)
        .env("NESSA_STAGE", "ci")
        .output()
        .unwrap()
}

fn stderr(run: &Run) -> String {
    String::from_utf8_lossy(&run.output.stderr).into_owned()
}

/// Display may use an equivalent OS spelling, so compare the complete resolved
/// identity. The structured audit assertion below separately requires the
/// original lossless target spelling.
fn assert_human_target(message: &str, target: &Path) {
    let displayed = message
        .split_once("credential registry at ")
        .and_then(|(_, rest)| rest.split_once(" was refused:").map(|(path, _)| path))
        .unwrap_or_else(|| panic!("diagnostic did not include a refused target: {message}"));
    assert_eq!(
        fs::canonicalize(displayed).unwrap(),
        fs::canonicalize(target).unwrap(),
        "{message}"
    );
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
    assert_human_target(&message, &run.registry);
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
    assert_human_target(&message, &run.registry);
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
    assert_human_target(&message, target);
    let records = records(&run.auth);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["target"]["value"], target.to_str().unwrap());
    assert_eq!(records[0]["fault"]["kind"], "unsafe_storage");
    assert_eq!(records[0]["fault"]["role"], role);
    let expected_transition = if role == "lock" {
        (
            "registry_lock_present_untrusted",
            "registry_lock_open_refused_file_preserved",
        )
    } else {
        (
            "registry_present_untrusted",
            "registry_open_refused_file_preserved",
        )
    };
    assert_eq!(records[0]["transition"]["before"], expected_transition.0);
    assert_eq!(records[0]["transition"]["after"], expected_transition.1);
}

#[cfg(unix)]
#[test]
fn public_registry_permissions_are_refused_and_audited_without_rewrite() {
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
fn unsafe_lock_preserves_an_initialized_registry_byte_for_byte() {
    let root = tempfile::tempdir().unwrap();
    let auth = root.path().join("ci/auth");
    let registry = auth.join("credentials.v1.json");
    let token = root.path().join("owner.token");
    let initialized = execute(
        root.path(),
        &[
            "auth".into(),
            "init".into(),
            "--local".into(),
            "--owner-token-file".into(),
            token.to_str().unwrap().into(),
        ],
    );
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let before = fs::read(&registry).unwrap();
    let lock = lock_path(&auth);
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
    let output = execute(root.path(), &["server".into()]);
    let run = Run {
        _root: root,
        registry,
        auth,
        output,
    };

    assert_unsafe_refusal(&run, &lock, "lock");
    assert_eq!(fs::read(&run.registry).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn unsafe_lock_does_not_create_an_absent_registry() {
    let root = tempfile::tempdir().unwrap();
    let auth = root.path().join("ci/auth");
    nessa_local_storage::create_directory(&auth).unwrap();
    let registry = auth.join("credentials.v1.json");
    let lock = lock_path(&auth);
    nessa_local_storage::open(&lock, nessa_local_storage::OpenMode::CreateNew).unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
    let output = execute(root.path(), &["server".into()]);
    let run = Run {
        _root: root,
        registry,
        auth,
        output,
    };

    assert_unsafe_refusal(&run, &lock, "lock");
    assert!(!run.registry.exists());
}

#[cfg(unix)]
#[test]
fn audit_ancestry_symlink_reports_sink_failure_without_writing_outside_root() {
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
