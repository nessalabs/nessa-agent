use super::{authorizes_rebootstrap, clear, publish, FILE_NAME};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt};

const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GENERATION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SERVICE: &str = "gui/501/so.nessa.gateway.prod";

fn definition(program: &str) -> Value {
    json!({
        "Label": "so.nessa.gateway.prod",
        "ProgramArguments": [program, "server"],
        "EnvironmentVariables": {
            "NESSA_RUNTIME_FINGERPRINT": FINGERPRINT,
            "NESSA_SERVICE_GENERATION": GENERATION
        }
    })
}

fn directory(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "nessa-install-attempt-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    nessa_local_storage::create_directory(&directory).unwrap();
    directory
}

#[test]
fn exact_host_attempt_and_both_definitions_are_required() {
    let directory = directory("agreement");
    let desired = definition("/runtime/nessa");
    publish(&directory, SERVICE, &desired).unwrap();

    assert!(authorizes_rebootstrap(&directory, SERVICE, &desired, Some(&desired)).unwrap());
    assert!(
        !authorizes_rebootstrap(&directory, "gui/501/other", &desired, Some(&desired)).unwrap()
    );
    assert!(!authorizes_rebootstrap(&directory, SERVICE, &desired, None).unwrap());

    let different_disk = definition("/foreign/nessa");
    assert!(!authorizes_rebootstrap(&directory, SERVICE, &desired, Some(&different_disk)).unwrap());
    assert!(
        !authorizes_rebootstrap(&directory, SERVICE, &different_disk, Some(&different_disk))
            .unwrap()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn private_attempt_is_atomic_validated_and_durably_cleared() {
    let directory = directory("storage");
    let desired = definition("/runtime/nessa");
    publish(&directory, SERVICE, &desired).unwrap();
    let path = directory.join(FILE_NAME);
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);

    fs::write(&path, b"malformed").unwrap();
    assert!(authorizes_rebootstrap(&directory, SERVICE, &desired, Some(&desired)).is_err());
    clear(&directory).unwrap();
    assert!(!path.exists());
    clear(&directory).unwrap();
    assert!(!authorizes_rebootstrap(&directory, SERVICE, &desired, Some(&desired)).unwrap());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn invalid_or_changed_identity_never_authorizes_rebootstrap() {
    let directory = directory("identity");
    let desired = definition("/runtime/nessa");
    publish(&directory, SERVICE, &desired).unwrap();
    for field in ["NESSA_RUNTIME_FINGERPRINT", "NESSA_SERVICE_GENERATION"] {
        let mut changed = desired.clone();
        changed["EnvironmentVariables"][field] = json!("c".repeat(64));
        assert!(!authorizes_rebootstrap(&directory, SERVICE, &changed, Some(&changed)).unwrap());
    }
    let mut invalid = desired;
    invalid["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] = json!("invalid");
    assert!(publish(&directory, SERVICE, &invalid).is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn install_attempt_read_rejects_a_fifo_without_waiting_for_a_writer() {
    let directory = directory("fifo");
    let path = directory.join(FILE_NAME);
    assert!(std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let desired = definition("/runtime/nessa");
    assert!(authorizes_rebootstrap(&directory, SERVICE, &desired, Some(&desired)).is_err());
    fs::remove_file(path).unwrap();
    fs::remove_dir_all(directory).unwrap();
}
