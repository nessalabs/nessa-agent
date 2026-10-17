//! The standalone binary verifies synthetic exports without bootstrapping a profile.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nessa_local_storage::create_directory;
use nessa_server as server;
use serde_json::Value;

#[path = "backup/verification_fixture.rs"]
mod fixture;
use fixture::{private_file, Fixture, BODY};

fn arguments(fixture: &Fixture) -> Vec<OsString> {
    vec![
        "--bundle".into(),
        fixture.bundle.clone().into_os_string(),
        "--mac-key-file".into(),
        fixture.key_file.clone().into_os_string(),
    ]
}

fn run(args: &[OsString], canary: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nessa-backup-verify"))
        .args(args)
        .env_clear()
        .env("HOME", canary)
        .env("NESSA_DATA_DIR", canary)
        .env("NESSA_STAGE", "synthetic")
        .env("XDG_CONFIG_HOME", canary)
        .env("XDG_DATA_HOME", canary)
        .env("XDG_STATE_HOME", canary)
        .env("NESSA_AGENT_PATH", canary.join("historical-execution"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("standalone verifier blocked or started long-lived work: {output:?}");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn json(output: &Output, exit: i32) -> Value {
    assert_eq!(output.status.code(), Some(exit));
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty: {:?}",
        output.stderr
    );
    assert!(output.stdout.ends_with(b"\n"));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn a_published_cut_verifies_without_profile_effects() {
    let fixture = Fixture::published().await;
    let canary = fixture.parent.path().join("synthetic-profile");
    for directory in [
        "synthetic/auth",
        "synthetic/logs",
        ".nessa",
        "synthetic/conversations",
    ] {
        create_directory(&canary.join(directory)).unwrap();
    }
    let files = [
        "synthetic/auth/credentials.v1.json",
        "synthetic/auth/identity.key",
        "synthetic/logs/gateway.log",
        "synthetic/config.json",
        ".nessa/config.json",
        "synthetic/conversations/records.sqlite3",
        "historical-execution",
    ];
    for file in files {
        private_file(
            &canary.join(file),
            b"SYNTHETIC-CANARY-DO-NOT-READ-OR-EXECUTE",
        );
    }
    let before = tree(&canary);
    let report = json(&run(&arguments(&fixture), &canary), 0);
    assert_eq!(report["status"], "verified");
    assert_eq!(report["report"]["public_records_verified"], 1);
    assert_eq!(report["report"]["body_bytes_verified"], BODY.len());
    assert_eq!(report["report"]["restore_readiness"], "not_assessed");
    assert_eq!(report["report"]["inventory_freshness"], "not_assessed");
    assert_eq!(report["report"]["omissions"][0]["identity"], "redacted-row");
    assert_eq!(tree(&canary), before);
    assert_eq!(fs::read(fixture.body()).unwrap(), BODY);
    assert_eq!(fs::read(&fixture.key_file).unwrap(), fixture.key.0);
    assert!(!fixture.store().join("redacted-row").exists());
}

#[tokio::test]
async fn an_empty_published_cut_verifies_without_profile_effects() {
    let fixture = Fixture::empty().await;
    let before = tree(fixture.parent.path());
    let report = json(&run(&arguments(&fixture), fixture.parent.path()), 0);
    assert_eq!(report["status"], "verified");
    assert_eq!(report["report"]["public_records_verified"], 0);
    assert_eq!(report["report"]["body_bytes_verified"], 0);
    assert_eq!(report["report"]["omissions"], serde_json::json!([]));
    assert_eq!(report["report"]["restore_readiness"], "not_assessed");
    assert_eq!(report["report"]["inventory_freshness"], "not_assessed");
    assert_eq!(tree(fixture.parent.path()), before);
    assert!(!fixture.bundle.join("records").exists());
}

fn tree(path: &Path) -> Vec<(OsString, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, result: &mut Vec<(OsString, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().as_os_str().to_owned();
            if path.is_dir() {
                result.push((relative, Vec::new()));
                visit(root, &path, result);
            } else {
                result.push((relative, fs::read(path).unwrap()));
            }
        }
    }
    let mut result = Vec::new();
    visit(path, path, &mut result);
    result.sort();
    result
}

#[tokio::test]
async fn command_reports_authentication_body_and_missing_bundle_failures() {
    let fixture = Fixture::published().await;
    fs::write(&fixture.key_file, [38; 32]).unwrap();
    let report = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
    assert_eq!(report["status"], "failed");
    assert_eq!(report["reason"], "authentication");
    fs::write(&fixture.key_file, fixture.key.0).unwrap();
    fs::write(fixture.body(), b"TAMPERED-SYNTHETIC").unwrap();
    let report = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
    assert_eq!(report["reason"], "body_fingerprint");
    fs::remove_dir_all(&fixture.bundle).unwrap();
    let report = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
    assert_eq!(report["reason"], "unreadable");
    assert!(!fixture.bundle.exists());
}

#[tokio::test]
async fn command_never_creates_or_repairs_invalid_key_files() {
    let fixture = Fixture::published().await;
    for size in [0, 31, 33, 8192] {
        let bytes = vec![53; size];
        fs::write(&fixture.key_file, &bytes).unwrap();
        let report = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
        assert_eq!(report["reason"], "invalid_key");
        assert_eq!(fs::read(&fixture.key_file).unwrap(), bytes);
    }
    fs::remove_file(&fixture.key_file).unwrap();
    let report = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
    assert_eq!(report["reason"], "unreadable");
    assert!(!fixture.key_file.exists());
}

#[tokio::test]
async fn command_requires_explicit_absolute_inputs_and_refuses_duplicate_unknown_flags() {
    let fixture = Fixture::published().await;
    let base = arguments(&fixture);
    let mut cases = vec![vec![], vec!["--bundle".into()], vec!["verify".into()]];
    for index in [1, 3] {
        let mut relative = base.clone();
        relative[index] = "relative-input".into();
        cases.push(relative);
    }
    for flag in ["--bundle", "--mac-key-file", "--profile", "--unknown"] {
        cases.push(vec![
            flag.into(),
            fixture.bundle.clone().into_os_string(),
            flag.into(),
            fixture.key_file.clone().into_os_string(),
        ]);
    }
    let mut extra = base.clone();
    extra.push("--help".into());
    cases.push(extra);
    for args in cases {
        assert_eq!(
            json(&run(&args, fixture.parent.path()), 2)["status"],
            "usage"
        );
    }
    let reverse = vec![
        base[2].clone(),
        base[3].clone(),
        base[0].clone(),
        base[1].clone(),
    ];
    assert_eq!(
        json(&run(&reverse, fixture.parent.path()), 0)["status"],
        "verified"
    );
    let help = run(&["--help".into()], fixture.parent.path());
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .starts_with("Usage: nessa-backup-verify"));
}

#[cfg(unix)]
mod unix {
    use std::ffi::CString;
    use std::io::ErrorKind;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixListener;

    use super::*;

    fn fifo(path: &Path) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // A private synthetic FIFO, deliberately without a writer.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    }

    #[tokio::test]
    async fn fifos_at_manifest_body_and_key_fail_without_waiting_for_a_writer() {
        for object in ["manifest", "body", "key"] {
            let fixture = Fixture::published().await;
            let path = match object {
                "manifest" => fixture.bundle.join("manifest.txt"),
                "body" => fixture.body(),
                "key" => fixture.key_file.clone(),
                _ => unreachable!(),
            };
            fs::remove_file(&path).unwrap();
            fifo(&path);
            let result = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
            assert_eq!(result["status"], "failed");
            assert_eq!(result["reason"], "unsafe_object");
            assert!(fs::symlink_metadata(&path).unwrap().file_type().is_fifo());
        }
    }

    #[tokio::test]
    async fn socket_body_is_refused_without_connecting_or_replacing_it() {
        let fixture = Fixture::published().await;
        fs::remove_file(fixture.body()).unwrap();
        let listener = UnixListener::bind(fixture.body()).unwrap();
        listener.set_nonblocking(true).unwrap();
        let result = json(&run(&arguments(&fixture), fixture.parent.path()), 1);
        assert_eq!(result["reason"], "unsafe_object");
        assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    }
}
