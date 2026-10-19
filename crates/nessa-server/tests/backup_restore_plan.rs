//! Real offline CLI: authentic synthetic bytes are inspected, never activated.

use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Result, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nessa_local_storage::create_directory;
use nessa_server as server;
use serde_json::{json, Value};

use server::backup::application::{Authenticator, ManifestPublisher, PublishedRecord};
use server::backup::domain::{
    AuthenticatedManifest, CapturedRecord, DeferredEntry, Fingerprint, Manifest, Sensitivity,
    SkipReason, SkippedRecord, SourceCut, StoreKind, Watermark, WRITER_INVENTORY,
};
use server::backup::infrastructure::DirectoryPublisher;

#[path = "backup/verification_fixture.rs"]
mod fixture;
use fixture::{private_file, Fixture, BODY};

#[path = "backup/planning_assertions.rs"]
mod planning_assertions;
use planning_assertions::assert_unavailable;

fn args(fixture: &Fixture) -> Vec<OsString> {
    vec![
        "--bundle".into(),
        fixture.bundle.clone().into_os_string(),
        "--mac-key-file".into(),
        fixture.key_file.clone().into_os_string(),
    ]
}

fn run(arguments: &[OsString], profile: &Path) -> Output {
    let trap = TcpListener::bind("127.0.0.1:0").unwrap();
    trap.set_nonblocking(true).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nessa-backup-plan"))
        .args(arguments)
        .env_clear()
        .env("HOME", profile)
        .env("NESSA_DATA_DIR", profile)
        .env("XDG_CONFIG_HOME", profile)
        .env("XDG_DATA_HOME", profile)
        .env("XDG_STATE_HOME", profile)
        .env("NESSA_STAGE", "synthetic")
        .env("NESSA_PORT", trap.local_addr().unwrap().port().to_string())
        .env("NESSA_AGENT_PATH", profile.join("historical-command"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!(
                "planner started long-lived work: {:?}",
                child.wait_with_output()
            );
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(trap.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
    child.wait_with_output().unwrap()
}

fn report(output: Output, exit: i32) -> Value {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn tree(root: &Path) -> Vec<(OsString, Vec<u8>)> {
    fn walk(root: &Path, path: &Path, rows: &mut Vec<(OsString, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let name = path.strip_prefix(root).unwrap().as_os_str().to_owned();
            if path.is_dir() {
                rows.push((name, Vec::new()));
                walk(root, &path, rows);
            } else {
                rows.push((name, fs::read(path).unwrap()));
            }
        }
    }
    let mut rows = Vec::new();
    walk(root, root, &mut rows);
    rows.sort();
    rows
}

#[tokio::test]
async fn authentic_and_empty_cuts_never_become_activation_authority() {
    for fixture in [Fixture::published().await, Fixture::empty().await] {
        let before = tree(fixture._parent.path());
        let value = report(run(&args(&fixture), fixture._parent.path()), 0);
        assert_unavailable(&value);
        assert_eq!(
            value["report"]["verification"]["inventory"]["status"],
            "not_included"
        );
        assert_eq!(tree(fixture._parent.path()), before);
        if fixture.body().exists() {
            assert_eq!(fs::read(fixture.body()).unwrap(), BODY);
        }
        assert!(!fixture.store().join("redacted-row").exists());
    }
}

#[tokio::test]
async fn authentic_historical_execution_bytes_and_omissions_remain_inert() {
    let fixture = Fixture::empty().await;
    fs::remove_dir_all(&fixture.bundle).unwrap();
    let profile = fixture._parent.path().join("profile");
    for directory in ["auth", "pairing", "leases", "outbox", "tools", "logs"] {
        create_directory(&profile.join(directory)).unwrap();
        private_file(
            &profile.join(directory).join("canary"),
            b"SYNTHETIC-UNCHANGED",
        );
    }
    private_file(
        &profile.join("historical-command"),
        b"SYNTHETIC-DO-NOT-EXECUTE",
    );
    let hostile = b"{\"command\":\"historical-command\",\"replay\":true,\"lease\":\"active\",\"paired\":true}";
    let mut records = Vec::new();
    let cuts = WRITER_INVENTORY
        .iter()
        .map(|kind| {
            let mut captured = Vec::new();
            if matches!(
                kind,
                StoreKind::LeaseRecords
                    | StoreKind::DeviceOutbox
                    | StoreKind::EnvironmentOutbox
                    | StoreKind::PeerPairings
            ) {
                captured.push(
                    CapturedRecord::new(*kind, "historical", Sensitivity::Public, hostile.to_vec())
                        .unwrap(),
                );
                records.push(PublishedRecord {
                    store: *kind,
                    identity: "historical".into(),
                    bytes: hostile.to_vec(),
                });
            }
            let (skipped, deferred) = if *kind == StoreKind::CanonicalRecords {
                (
                    vec![
                        SkippedRecord::new(*kind, "same-label", SkipReason::Unnameable).unwrap(),
                        SkippedRecord::new(*kind, "same-label", SkipReason::Unavailable).unwrap(),
                    ],
                    vec![DeferredEntry::new(
                        *kind,
                        "large-session",
                        65 * 1024 * 1024,
                        Fingerprint::of(b"not-retained"),
                    )
                    .unwrap()],
                )
            } else {
                (Vec::new(), Vec::new())
            };
            SourceCut::with_deferred(
                *kind,
                Watermark::new("synthetic").unwrap(),
                captured,
                skipped,
                deferred,
            )
            .unwrap()
        })
        .collect();
    let manifest = Manifest::new(456, cuts).unwrap();
    let mac = fixture.key.mac(&manifest.canonical()).unwrap();
    DirectoryPublisher::new(&fixture.bundle)
        .put(
            &AuthenticatedManifest::seal(manifest, mac).encoded(),
            &records,
        )
        .await
        .unwrap();
    let before = tree(fixture._parent.path());
    let value = report(run(&args(&fixture), &profile), 0);
    assert_unavailable(&value);
    assert_eq!(
        value["report"]["verification"]["public_records_verified"],
        4
    );
    assert_eq!(
        value["report"]["verification"]["omissions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        value["report"]["omission_summary"],
        json!([{"store":"canonical-records","rows":3,"redacted":0,"skipped":2,"deferred":1,"source_origins":"not_established"}])
    );
    assert!(value["report"]["unresolved_prerequisites"]
        .as_array()
        .unwrap()
        .contains(&json!("deferred_payload_availability")));
    assert_eq!(tree(fixture._parent.path()), before);
}

#[tokio::test]
async fn verifier_failures_never_emit_a_plan_or_change_inputs() {
    for failure in [
        "authentication",
        "body_fingerprint",
        "missing_record",
        "unsupported_format",
    ] {
        let fixture = Fixture::published().await;
        match failure {
            "authentication" => fs::write(&fixture.key_file, [99; 32]).unwrap(),
            "body_fingerprint" => fs::write(fixture.body(), b"tampered").unwrap(),
            "missing_record" => fs::remove_file(fixture.body()).unwrap(),
            _ => {
                let path = fixture.bundle.join("manifest.txt");
                let text = fs::read_to_string(&path).unwrap().replacen(
                    "nessa-export-cut/1",
                    "nessa-export-cut/2",
                    1,
                );
                fs::write(path, text).unwrap();
            }
        }
        let before = tree(fixture._parent.path());
        assert_eq!(
            report(run(&args(&fixture), fixture._parent.path()), 1),
            json!({"status":"failed","reason":failure})
        );
        assert_eq!(tree(fixture._parent.path()), before);
    }
}

#[tokio::test]
async fn no_activation_destination_policy_or_plan_replay_option_exists() {
    let fixture = Fixture::empty().await;
    let before = tree(fixture._parent.path());
    for option in [
        "--activate",
        "--destination",
        "--accept-stale",
        "--recover-identity",
        "--plan",
    ] {
        let mut arguments = args(&fixture);
        arguments.extend([
            option.into(),
            fixture._parent.path().join("forbidden").into_os_string(),
        ]);
        assert_eq!(
            report(run(&arguments, fixture._parent.path()), 2),
            json!({"status":"usage"})
        );
    }
    assert_eq!(tree(fixture._parent.path()), before);
    let help = run(&["--help".into()], fixture._parent.path());
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("Exit 0 means an inspection report"));
}

#[test]
fn output_failure_is_observable_without_bootstrap() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> Result<usize> {
            Err(ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        server::composition::run_restore_planner(&["--help".into()], &mut Broken),
        1
    );
    assert_eq!(
        server::composition::run_restore_planner(&[], &mut Broken),
        1
    );
}
