//! Files the cut reads: a bad line is named, a secret is not copied, and
//! both outboxes are part of the same cut.

use std::fs;
use std::path::Path;

use async_trait::async_trait;
use nessa_protocol::lease::MAX_ARTIFACT_BYTES;

use crate::backup::application::{
    verify_bundle, ExportClock, ExportFailure, ManifestPublisher, MutationAdmission,
    PublishedRecord, VerificationLimits,
};
use crate::backup::domain::{outbox_watermark, Sensitivity, SkipReason, StoreKind};
use crate::backup::infrastructure::{
    capture_namespace, jsonl_cut, path_sensitivity, DirectoryPublisher,
    DirectoryVerificationSource, FileDeletionLog,
};
use crate::conversation::application::ArtifactBudget;

#[test]
fn an_unreadable_jsonl_line_is_named_and_the_rest_are_kept() {
    let cut = jsonl_cut(
        StoreKind::LeaseRecords,
        "{\"lease\":1}\nnot json\n{\"lease\":2}\n",
    )
    .unwrap();
    assert_eq!(cut.records().len(), 2);
    assert_eq!(cut.skipped().len(), 1);
    assert_eq!(cut.skipped()[0].identity(), "line-2");
    assert_eq!(cut.skipped()[0].reason(), SkipReason::Unreadable);
    assert_eq!(cut.watermark().as_str(), "3");
}

#[test]
fn a_secret_path_is_not_public() {
    assert_eq!(
        path_sensitivity(Path::new("native-pairing/gateway-key")),
        Sensitivity::Secret
    );
    assert_eq!(
        path_sensitivity(Path::new("auth/credentials.v1.json")),
        Sensitivity::Secret
    );
    assert_eq!(
        path_sensitivity(Path::new("environment/pairing/client-enrollment")),
        Sensitivity::Secret
    );
    assert_eq!(
        path_sensitivity(Path::new("environment/leases.jsonl")),
        Sensitivity::Public
    );
    assert_eq!(
        path_sensitivity(Path::new("environment/grants.json")),
        Sensitivity::Public
    );
    assert_eq!(
        path_sensitivity(Path::new("environment/pairing/gateway-key-intent.json")),
        Sensitivity::Secret
    );
    assert_eq!(
        path_sensitivity(Path::new("environment/pairing/not-a-store")),
        Sensitivity::Secret
    );
}

#[tokio::test]
async fn current_writer_omissions_verify_without_publishing_secret_bytes() {
    const SECRET: &[u8] = b"-----BEGIN EC PRIVATE KEY-----\nSYNTHETIC-OMITTED-CONTENT\n";
    for mode in ["secret", "unnameable", "jsonl", "mixed-jsonl", "public"] {
        let namespace = tempfile::tempdir().unwrap();
        let root = namespace.path();
        let (store, omissions) = if matches!(mode, "secret" | "public") {
            let sessions = root.join("conversations/sessions");
            fs::create_dir_all(&sessions).unwrap();
            fs::write(
                sessions.join("records.sqlite3"),
                if mode == "secret" {
                    SECRET
                } else {
                    b"synthetic public session"
                },
            )
            .unwrap();
            (
                StoreKind::CanonicalRecords,
                if mode == "secret" { 2 } else { 0 },
            )
        } else {
            let mut audit = root.join("conversations/audit/creation");
            if mode == "unnameable" {
                for _ in 0..6 {
                    audit = audit.join("n".repeat(100));
                }
                fs::create_dir_all(&audit).unwrap();
                fs::write(audit.join("a.txt"), b"synthetic a").unwrap();
                fs::write(audit.join("b.txt"), b"synthetic b").unwrap();
            } else {
                for child in ["a", "b"] {
                    let directory = audit.join(child);
                    fs::create_dir_all(&directory).unwrap();
                    let body = if mode == "mixed-jsonl" && child == "a" {
                        b"{}\n".as_slice()
                    } else {
                        b"synthetic-invalid-json\n"
                    };
                    fs::write(directory.join("leases.jsonl"), body).unwrap();
                }
            }
            (
                StoreKind::ConversationAudit,
                if mode == "mixed-jsonl" { 1 } else { 2 },
            )
        };
        let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
        let output = tempfile::tempdir().unwrap();
        let bundle = output.path().join("cut");
        capture_namespace(
            root,
            &OpenAdmission,
            &inventory,
            &DirectoryPublisher::new(&bundle),
            &Clock(8),
            &inventory.key(),
        )
        .await
        .unwrap();
        let report = verify_bundle(
            &DirectoryVerificationSource::new(bundle.clone()),
            &inventory.key(),
            VerificationLimits::default(),
        )
        .unwrap();
        let rows: Vec<_> = report
            .omissions
            .iter()
            .filter(|row| row.store == store.as_str())
            .collect();
        assert_eq!(rows.len(), omissions, "{mode}");
        if mode == "secret" {
            assert!(rows.iter().any(|row| row.kind == "redacted"));
            assert!(rows.iter().any(|row| row.reason == Some("secret")));
        }
        if mode == "mixed-jsonl" {
            assert_eq!(rows[0].identity, "line-1");
            assert_eq!(
                fs::read(bundle.join("records/conversation-audit/line-1")).unwrap(),
                b"{}"
            );
        }
        assert!(!holds(&read_tree(&bundle), SECRET), "{mode}");
        assert!(
            !holds(&serde_json::to_vec(&report).unwrap(), SECRET),
            "{mode}"
        );
        assert_eq!(report.restore_readiness, "not_assessed");
    }
}

#[tokio::test]
async fn a_secret_is_fingerprinted_and_left_out_of_the_bundle() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    fs::create_dir_all(root.join("environment/pairing")).unwrap();
    fs::create_dir_all(root.join("environment/outbox")).unwrap();
    fs::create_dir_all(root.join("auth")).unwrap();
    let secret = b"GATEWAY-PRIVATE-KEY-BYTES";
    fs::write(root.join("environment/pairing/gateway-key"), secret).unwrap();
    fs::write(
        root.join("environment/outbox/pending.txt"),
        b"OUTBOX-MARKER-NOT-A-BACKUP",
    )
    .unwrap();
    fs::write(
        root.join("environment/leases.jsonl"),
        b"{\"lease\":\"live\"}\nbad-line\n",
    )
    .unwrap();
    fs::write(
        root.join("config.json"),
        br#"{"native":{"listenAddress":"127.0.0.1:1","relay":"relay.example:443"},"token":"CONFIG-TOKEN"}"#,
    )
    .unwrap();
    let verifier = "VERIFIER-MUST-NOT-LEAK";
    fs::write(
        root.join("auth/credentials.v1.json"),
        format!(
            r#"{{"credentials":[{{"metadata":{{"id":"device-1","principalId":"user:one","organizationId":"org","audienceId":"gateway","issuedAt":1,"expiresAt":null,"revokedAt":9,"grants":[{{"action":"server.read","resource":{{"organizationId":"org","id":"gateway"}}}}]}},"verifier":"{verifier}"}}]}}"#
        ),
    )
    .unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let key = inventory.key().0;
    let mac = inventory.key();
    let bundle = tempfile::tempdir().unwrap();
    let publisher = DirectoryPublisher::new(bundle.path().join("cut"));
    let manifest = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &publisher,
        &Clock(8),
        &mac,
    )
    .await
    .unwrap();
    let text = fs::read_to_string(bundle.path().join("cut/manifest.txt")).unwrap();
    assert_eq!(text, manifest.encoded());
    let bundle_bytes = read_tree(&bundle.path().join("cut"));
    assert!(!bundle_bytes
        .windows(secret.len())
        .any(|window| window == secret));
    assert!(!bundle_bytes
        .windows(verifier.len())
        .any(|window| window == verifier.as_bytes()));
    assert!(!bundle_bytes.windows(key.len()).any(|window| window == key));
    let marker = b"OUTBOX-MARKER-NOT-A-BACKUP";
    assert!(bundle_bytes
        .windows(marker.len())
        .any(|window| window == marker));
    assert!(text.contains("watermark device-outbox absent"));
    assert!(text.contains(&format!(
        "watermark environment-outbox {}",
        outbox_watermark(&[("pending.txt", marker.len() as u64)])
            .unwrap()
            .as_str()
    )));
    assert!(!text.contains("CONFIG-TOKEN"));
    assert!(text.contains("redacted environment-peer-pairings"));
    assert!(text.contains("skipped lease-records line-2 unreadable"));
    assert!(text.contains("device-1"));
    let worker = manifest.manifest().store(StoreKind::WorkerCredentials);
    assert!(worker.records().is_empty());
    assert_eq!(worker.watermark().as_str(), "0");
    let device = manifest.manifest().store(StoreKind::DeviceCredentials);
    assert_eq!(device.records().len(), 1);
    assert!(device.records()[0]
        .public_bytes()
        .unwrap()
        .windows(verifier.len())
        .all(|window| window != verifier.as_bytes()));
    let relay = manifest.manifest().store(StoreKind::RelayConfiguration);
    let relay_body = relay.records()[0].public_bytes().unwrap();
    assert_eq!(relay_body, b"\"relay.example:443\"");
    assert!(!std::str::from_utf8(relay_body)
        .unwrap()
        .contains("CONFIG-TOKEN"));
}

#[tokio::test]
async fn a_concurrent_outbox_append_is_outside_the_cut() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let device = root.join("device/outbox");
    let environment = root.join("environment/outbox/aa");
    fs::create_dir_all(&device).unwrap();
    fs::create_dir_all(&environment).unwrap();
    let device_body = b"{\"id\":1}\n";
    let environment_body = b"staged-before";
    let device_file = device.join("intent.json");
    let environment_file = environment.join("0");
    fs::write(&device_file, device_body).unwrap();
    fs::write(&environment_file, environment_body).unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &AppendOnRelease {
            appends: vec![
                (device_file.clone(), b"-APPENDED".to_vec()),
                (environment_file.clone(), b"-APPENDED".to_vec()),
            ],
            creates: vec![(device.join("later.json"), b"CONCURRENT-ROW".to_vec())],
        },
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let text = manifest.encoded();
    let device_mark = outbox_watermark(&[("intent.json", device_body.len() as u64)]).unwrap();
    let environment_mark = outbox_watermark(&[("aa/0", environment_body.len() as u64)]).unwrap();
    let appended_device = outbox_watermark(&[(
        "intent.json",
        (device_body.len() + b"-APPENDED".len()) as u64,
    )])
    .unwrap();
    let with_later_row = outbox_watermark(&[
        ("intent.json", device_body.len() as u64),
        ("later.json", b"CONCURRENT-ROW".len() as u64),
    ])
    .unwrap();
    assert!(text.contains(&format!("watermark device-outbox {}", device_mark.as_str())));
    assert!(text.contains(&format!(
        "watermark environment-outbox {}",
        environment_mark.as_str()
    )));
    assert!(!text.contains(appended_device.as_str()));
    assert!(!text.contains(with_later_row.as_str()));
    let published = fs::read(bundle.path().join("cut/records/device-outbox/intent.json")).unwrap();
    assert_eq!(published, device_body);
    let staged = fs::read(bundle.path().join("cut/records/environment-outbox/aa%2F0")).unwrap();
    assert_eq!(staged, environment_body);
    let bundle_bytes = read_tree(&bundle.path().join("cut"));
    assert!(!holds(&bundle_bytes, b"-APPENDED"));
    assert!(!holds(&bundle_bytes, b"CONCURRENT-ROW"));
    assert_eq!(fs::read(&device_file).unwrap(), b"{\"id\":1}\n-APPENDED");
    assert!(device.join("later.json").is_file());
}

#[tokio::test]
async fn an_environment_outbox_cut_waits_for_the_writer_lock() {
    use std::time::Duration;

    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let outbox = root.join("environment/outbox/aa");
    fs::create_dir_all(&outbox).unwrap();
    fs::write(outbox.join("0"), b"PARTIAL-ROW").unwrap();
    let lock = nessa_local_storage::open(
        &root.join("environment/outbox.lock"),
        nessa_local_storage::OpenMode::OpenOrCreate,
    )
    .unwrap();
    lock.lock().unwrap();
    let captured = root.to_path_buf();
    let task = tokio::spawn(async move { publish_cut(&captured).await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !task.is_finished(),
        "the cut must wait while outbox.lock is held"
    );
    fs::write(outbox.join("0"), b"COMPLETE-ROW").unwrap();
    drop(lock);
    let published = task.await.unwrap();
    assert!(holds(&published.bytes, b"COMPLETE-ROW"));
    assert!(!holds(&published.bytes, b"PARTIAL-ROW"));
}

#[tokio::test]
async fn an_invalid_outbox_is_refused_and_nothing_is_committed() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let device = root.join("device/outbox");
    fs::create_dir_all(&device).unwrap();
    fs::write(device.join("intent.json"), b"{\"id\":1}\n").unwrap();
    fs::create_dir_all(root.join("environment")).unwrap();
    fs::write(root.join("environment/outbox"), b"not-a-directory").unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let log = root.join("backup/deletion-inventory.txt");
    assert!(!log.exists());
    let bundle = tempfile::tempdir().unwrap();
    let outcome = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await;
    assert_eq!(outcome, Err(ExportFailure::Outbox));
    assert!(!bundle.path().join("cut").exists());
    assert!(!log.exists());
    let tree = read_tree(bundle.path());
    assert!(!holds(&tree, b"{\"id\":1}"));

    refuse_a_dangling_outbox_link().await;
}

#[cfg(unix)]
fn link_missing_outbox(link: &std::path::Path) {
    std::os::unix::fs::symlink("missing-outbox", link).unwrap();
}

#[cfg(windows)]
fn link_missing_outbox(link: &std::path::Path) {
    std::os::windows::fs::symlink_file("missing-outbox", link).unwrap();
}

#[cfg(any(unix, windows))]
async fn refuse_a_dangling_outbox_link() {
    let linked = tempfile::tempdir().unwrap();
    let link_root = linked.path();
    fs::create_dir_all(link_root.join("environment")).unwrap();
    link_missing_outbox(&link_root.join("environment/outbox"));
    let linked_inventory = FileDeletionLog::open(link_root.join("backup")).unwrap();
    let linked_bundle = tempfile::tempdir().unwrap();
    let linked_outcome = capture_namespace(
        link_root,
        &OpenAdmission,
        &linked_inventory,
        &DirectoryPublisher::new(linked_bundle.path().join("cut")),
        &Clock(8),
        &linked_inventory.key(),
    )
    .await;
    assert_eq!(linked_outcome, Err(ExportFailure::Outbox));
    assert!(!linked_bundle.path().join("cut").exists());
}

#[cfg(not(any(unix, windows)))]
async fn refuse_a_dangling_outbox_link() {}

#[tokio::test]
async fn an_outbox_past_the_lease_budget_aborts_the_cut() {
    let files = usize::try_from(ArtifactBudget::LEASE.files).unwrap();
    let too_many = tempfile::tempdir().unwrap();
    stage_device_marker(too_many.path());
    let crowded = too_many.path().join("environment/outbox/aa");
    fs::create_dir_all(&crowded).unwrap();
    for index in 0..=files {
        fs::write(crowded.join(index.to_string()), b"x").unwrap();
    }
    assert_outbox_cut_refused(too_many.path()).await;

    let too_large = tempfile::tempdir().unwrap();
    stage_device_marker(too_large.path());
    let heavy = too_large.path().join("environment/outbox/aa");
    fs::create_dir_all(&heavy).unwrap();
    let mut left = ArtifactBudget::LEASE.bytes;
    let mut index = 0u32;
    while left > 0 {
        let len = left.min(MAX_ARTIFACT_BYTES);
        let file = fs::File::create(heavy.join(index.to_string())).unwrap();
        file.set_len(len).unwrap();
        left -= len;
        index += 1;
    }
    let extra = fs::File::create(heavy.join(index.to_string())).unwrap();
    extra.set_len(1).unwrap();
    assert!(
        usize::try_from(index).unwrap() < files,
        "the byte case has to stay inside the file budget"
    );
    assert_outbox_cut_refused(too_large.path()).await;
}

fn stage_device_marker(root: &Path) {
    let device = root.join("device/outbox");
    fs::create_dir_all(&device).unwrap();
    fs::write(device.join("intent.json"), b"DEVICE-MARKER-NOT-PUBLISHED").unwrap();
}

async fn assert_outbox_cut_refused(root: &Path) {
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let log = root.join("backup/deletion-inventory.txt");
    assert!(!log.exists());
    let bundle = tempfile::tempdir().unwrap();
    let outcome = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await;
    assert_eq!(outcome, Err(ExportFailure::Outbox));
    assert!(!bundle.path().join("cut").exists());
    assert!(!log.exists());
    assert!(!holds(
        &read_tree(bundle.path()),
        b"DEVICE-MARKER-NOT-PUBLISHED"
    ));
}

#[tokio::test]
async fn an_outbox_private_key_is_fingerprinted_and_the_cut_continues() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let outbox = root.join("device/outbox");
    fs::create_dir_all(&outbox).unwrap();
    let secret = b"-----BEGIN OPENSSH PRIVATE KEY-----\n";
    fs::write(outbox.join("intent.json"), secret).unwrap();
    fs::create_dir_all(root.join("environment/outbox")).unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let text = manifest.encoded();
    assert!(text.contains("redacted device-outbox intent.json"));
    assert!(!text.contains("skipped device-outbox"));
    let bundle_bytes = read_tree(&bundle.path().join("cut"));
    assert!(!holds(&bundle_bytes, secret));
    assert!(text.contains(&format!(
        "watermark device-outbox {}",
        outbox_watermark(&[("intent.json", secret.len() as u64)])
            .unwrap()
            .as_str()
    )));
    assert!(text.contains(&format!(
        "watermark environment-outbox {}",
        outbox_watermark(&[]).unwrap().as_str()
    )));
    assert!(!text.contains("watermark environment-outbox absent"));
}

#[tokio::test]
async fn an_unknown_gateway_file_is_not_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    fs::create_dir_all(root.join("environment/pairing")).unwrap();
    let secret = b"UNKNOWN-FILE-MUST-NOT-BE-COPIED";
    fs::write(root.join("environment/pairing/not-a-store"), secret).unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let bundle_bytes = read_tree(&bundle.path().join("cut"));
    assert!(!bundle_bytes
        .windows(secret.len())
        .any(|window| window == secret));
    let text = fs::read_to_string(bundle.path().join("cut/manifest.txt")).unwrap();
    assert!(text.contains("redacted environment-peer-pairings not-a-store"));
    let pairing = manifest
        .manifest()
        .store(StoreKind::EnvironmentPeerPairings);
    assert!(pairing.records()[0].public_bytes().is_none());
}

#[tokio::test]
async fn a_session_database_without_secret_material_is_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute_batch("CREATE TABLE note(body TEXT); INSERT INTO note VALUES ('hello-note');")
        .unwrap();
    drop(database);
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let canonical = manifest.manifest().store(StoreKind::CanonicalRecords);
    let body = canonical.records()[0].public_bytes().unwrap();
    assert!(body
        .windows(b"hello-note".len())
        .any(|window| window == b"hello-note"));
}

#[tokio::test]
async fn a_session_database_holding_a_private_key_is_not_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let secret = b"-----BEGIN OPENSSH PRIVATE KEY-----";
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute("CREATE TABLE payload(body BLOB)", [])
        .unwrap();
    database
        .execute("INSERT INTO payload(body) VALUES (?1)", [secret])
        .unwrap();
    drop(database);
    let published = publish_cut(root).await;
    assert!(!holds(&published.bytes, secret));
    assert!(published
        .text
        .contains("redacted canonical-records records.sqlite3"));
    assert!(published
        .text
        .contains("skipped canonical-records records.sqlite3 secret"));
    assert!(published
        .manifest
        .manifest()
        .store(StoreKind::CanonicalRecords)
        .records()[0]
        .public_bytes()
        .is_none());
}

#[tokio::test]
async fn a_session_database_holding_an_rsa_ec_or_encrypted_private_key_is_not_copied() {
    let headers: &[&[u8]] = &[
        b"-----BEGIN RSA PRIVATE KEY-----",
        b"-----BEGIN EC PRIVATE KEY-----",
        b"-----BEGIN ENCRYPTED PRIVATE KEY-----",
    ];
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute("CREATE TABLE payload(body BLOB)", [])
        .unwrap();
    for header in headers {
        database
            .execute("INSERT INTO payload(body) VALUES (?1)", [*header])
            .unwrap();
    }
    drop(database);
    let published = publish_cut(root).await;
    for header in headers {
        assert!(
            !holds(&published.bytes, header),
            "published {}",
            String::from_utf8_lossy(header)
        );
    }
    assert!(published
        .text
        .contains("redacted canonical-records records.sqlite3"));
    assert!(published
        .text
        .contains("skipped canonical-records records.sqlite3 secret"));
}

#[tokio::test]
async fn a_plaintext_public_file_holding_a_private_key_is_not_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let secret = b"-----BEGIN EC PRIVATE KEY-----\nEC-KEY-BODY-MUST-NOT-PUBLISH\n";
    let directory = root.join("conversations/audit/creation");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json"),
        secret,
    )
    .unwrap();
    let published = publish_cut(root).await;
    assert!(!holds(&published.bytes, b"EC-KEY-BODY-MUST-NOT-PUBLISH"));
    assert!(!holds(&published.bytes, b"-----BEGIN EC PRIVATE KEY-----"));
    assert!(published.text.contains(
        "redacted conversation-audit creation%2Faaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json"
    ));
    assert!(published.text.contains(
        "skipped conversation-audit creation%2Faaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json secret"
    ));
}

#[tokio::test]
async fn creation_and_deletion_audits_keep_separate_identities() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let creation = root.join("conversations/audit/creation");
    let deletion = root.join("conversations/audit/deletion");
    fs::create_dir_all(&creation).unwrap();
    fs::create_dir_all(&deletion).unwrap();
    let name = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json";
    fs::write(creation.join(name), b"CREATION-AUDIT-BODY").unwrap();
    fs::write(deletion.join(name), b"DELETION-AUDIT-BODY").unwrap();
    let published = publish_cut(root).await;
    assert_eq!(
        fs::read(
            published
                .root
                .join("records/conversation-audit")
                .join(format!("creation%2F{name}"))
        )
        .unwrap(),
        b"CREATION-AUDIT-BODY"
    );
    assert_eq!(
        fs::read(
            published
                .root
                .join("records/conversation-audit")
                .join(format!("deletion%2F{name}"))
        )
        .unwrap(),
        b"DELETION-AUDIT-BODY"
    );
}

#[tokio::test]
async fn an_environment_grant_cut_waits_for_the_writer_lock() {
    use std::time::Duration;

    let namespace = tempfile::tempdir().unwrap();
    let environment = namespace.path().join("environment");
    fs::create_dir_all(&environment).unwrap();
    fs::write(environment.join("grants.json"), b"{\"cut\":\"before\"}\n").unwrap();
    fs::write(
        environment.join("grants-audit.jsonl"),
        b"{\"cut\":\"before\"}\n",
    )
    .unwrap();
    let lock = nessa_local_storage::open(
        &environment.join("grants.lock"),
        nessa_local_storage::OpenMode::OpenOrCreate,
    )
    .unwrap();
    lock.lock().unwrap();
    let root = namespace.path().to_path_buf();
    let task = tokio::spawn(async move { publish_cut(&root).await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !task.is_finished(),
        "the cut must wait while grants.lock is held"
    );
    fs::write(environment.join("grants.json"), b"{\"cut\":\"after\"}\n").unwrap();
    fs::write(
        environment.join("grants-audit.jsonl"),
        b"{\"cut\":\"after\"}\n",
    )
    .unwrap();
    drop(lock);
    let published = task.await.unwrap();
    assert!(holds(&published.bytes, b"\"cut\":\"after\""));
    assert!(!holds(&published.bytes, b"\"cut\":\"before\""));
}

#[tokio::test]
async fn a_publication_lock_snapshots_the_credential_and_the_receiver_together() {
    use std::time::Duration;

    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let access = root.join("receiver-access");
    fs::create_dir_all(&access).unwrap();
    fs::create_dir_all(root.join("auth")).unwrap();
    write_registry(root, "before-device");
    write_receiver(&access, "before-receiver", "before-device");
    let lock = nessa_local_storage::open(
        &access.join("publication.lock"),
        nessa_local_storage::OpenMode::OpenOrCreate,
    )
    .unwrap();
    lock.lock().unwrap();
    let captured = root.to_path_buf();
    let task = tokio::spawn(async move { publish_cut(&captured).await });
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !task.is_finished(),
        "the cut must wait while publication.lock is held"
    );
    write_registry(root, "after-device");
    write_receiver(&access, "after-receiver", "after-device");
    drop(lock);
    let published = task.await.unwrap();
    assert!(holds(&published.bytes, b"after-device"));
    assert!(holds(&published.bytes, b"after-receiver"));
    assert!(!holds(&published.bytes, b"before-device"));
    assert!(!holds(&published.bytes, b"before-receiver"));
}

fn write_registry(root: &Path, id: &str) {
    let registry = serde_json::json!({
        "credentials": [{
            "metadata": {
                "id": id,
                "principalId": "user:one",
                "organizationId": "org",
                "audienceId": "gateway",
                "issuedAt": 1,
                "expiresAt": null,
                "revokedAt": null,
                "grants": [{"action": "server.read", "resource": {"organizationId": "org", "id": "gateway"}}]
            },
            "verifier": "SECRET-VERIFIER"
        }]
    });
    fs::write(
        root.join("auth/credentials.v1.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
}

fn write_receiver(access: &Path, receiver_id: &str, credential_id: &str) {
    let path = access.join("receiver-access.sqlite3");
    let _ = fs::remove_file(&path);
    let database = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch(
            "CREATE TABLE receiver_transitions(sequence INTEGER);
             CREATE TABLE receivers(
                receiver_id TEXT,
                credential_id TEXT,
                access_epoch INTEGER,
                active INTEGER
             );",
        )
        .unwrap();
    database
        .execute("INSERT INTO receiver_transitions(sequence) VALUES (1)", [])
        .unwrap();
    database
        .execute(
            "INSERT INTO receivers(receiver_id, credential_id, access_epoch, active)
             VALUES (?1, ?2, 1, 1)",
            [receiver_id, credential_id],
        )
        .unwrap();
}

#[tokio::test]
async fn a_public_file_over_the_bound_is_named_and_not_read() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let environment = root.join("environment");
    fs::create_dir_all(&environment).unwrap();
    let audit = environment.join("grants-audit.jsonl");
    let file = fs::File::create(&audit).unwrap();
    file.set_len(crate::backup::infrastructure::snapshot::PUBLIC_FILE_LIMIT + 1)
        .unwrap();
    drop(file);
    fs::write(
        environment.join("leases.jsonl"),
        b"{\"lease\":\"still-cut\"}\n",
    )
    .unwrap();
    let published = publish_cut(root).await;
    assert!(published
        .text
        .contains("skipped environment-audit grants-audit.jsonl bounded"));
    assert!(holds(&published.bytes, b"still-cut"));
    assert!(!published
        .root
        .join("records/environment-audit/grants-audit.jsonl")
        .exists());
}

#[tokio::test]
async fn an_oversized_non_database_session_is_not_read() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join("records.sqlite3");
    fs::write(&path, b"RAW-SESSION-MUST-NOT-PUBLISH").unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_len(crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT + 1)
        .unwrap();
    drop(file);
    let published = publish_cut(root).await;
    assert!(published
        .text
        .contains("skipped canonical-records records.sqlite3 unreadable"));
    assert!(!holds(&published.bytes, b"RAW-SESSION-MUST-NOT-PUBLISH"));
    assert!(!published
        .root
        .join("records/canonical-records/records.sqlite3")
        .exists());

    let small = tempfile::tempdir().unwrap();
    let small_root = small.path();
    let small_sessions = small_root.join("conversations/sessions");
    fs::create_dir_all(&small_sessions).unwrap();
    fs::write(
        small_sessions.join("records.sqlite3"),
        b"tiny-not-sqlite-kept",
    )
    .unwrap();
    let copied = publish_cut(small_root).await;
    assert!(holds(&copied.bytes, b"tiny-not-sqlite-kept"));
}

#[tokio::test]
async fn a_malformed_peer_relay_is_named_skipped() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let peers = root.join("peer-gateways");
    fs::create_dir_all(&peers).unwrap();
    fs::write(peers.join("bad.json"), b"{").unwrap();
    fs::write(peers.join("good.json"), br#"{"relay":"kept-relay"}"#).unwrap();
    let published = publish_cut(root).await;
    assert!(published
        .text
        .contains("skipped relay-configuration bad.json unreadable"));
    assert!(holds(&published.bytes, b"kept-relay"));
}

#[tokio::test]
async fn a_json_record_holding_an_rsa_private_key_is_not_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    fs::create_dir_all(root.join("environment")).unwrap();
    let secret = b"-----BEGIN RSA PRIVATE KEY-----";
    fs::write(
        root.join("environment/grants.json"),
        format!(r#"{{"holder":"{}"}}"#, String::from_utf8_lossy(secret)),
    )
    .unwrap();
    let published = publish_cut(root).await;
    assert!(!holds(&published.bytes, secret));
    assert!(published
        .text
        .contains("skipped environment-grants grants.json secret"));
}

#[tokio::test]
async fn an_undashed_private_key_phrase_is_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let phrase = b"BEGIN RSA PRIVATE KEY";
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute("CREATE TABLE payload(body BLOB)", [])
        .unwrap();
    database
        .execute("INSERT INTO payload(body) VALUES (?1)", [phrase])
        .unwrap();
    drop(database);
    let published = publish_cut(root).await;
    let canonical = fs::read(
        published
            .root
            .join("records/canonical-records/records.sqlite3"),
    )
    .unwrap();
    assert!(holds(&canonical, phrase));
}

#[tokio::test]
async fn a_public_jsonl_holding_a_private_key_is_not_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    fs::create_dir_all(root.join("environment")).unwrap();
    let secret = b"-----BEGIN ENCRYPTED PRIVATE KEY-----";
    fs::write(
        root.join("environment/leases.jsonl"),
        format!("{{\"note\":\"{}\"}}\n", String::from_utf8_lossy(secret)),
    )
    .unwrap();
    let published = publish_cut(root).await;
    assert!(!holds(&published.bytes, secret));
    assert!(published
        .text
        .contains("skipped lease-records leases.jsonl secret"));
}

#[tokio::test]
async fn a_session_database_quoting_token_is_still_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute_batch(
            "CREATE TABLE note(body TEXT);
             INSERT INTO note VALUES ('{\"token\":\"SESSION-TOKEN-STAYS\"}');
             INSERT INTO note VALUES ('{\"secret\":\"ORDINARY-SECRET-WORD\"}');",
        )
        .unwrap();
    drop(database);
    let published = publish_cut(root).await;
    assert!(holds(&published.bytes, b"SESSION-TOKEN-STAYS"));
    assert!(holds(&published.bytes, b"ORDINARY-SECRET-WORD"));
    assert!(!published
        .text
        .contains("skipped canonical-records records.sqlite3 secret"));
}

#[tokio::test]
async fn a_public_key_header_in_a_session_database_is_copied() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let header = b"-----BEGIN PUBLIC KEY-----\n-----BEGIN CERTIFICATE-----";
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute("CREATE TABLE payload(body BLOB)", [])
        .unwrap();
    database
        .execute("INSERT INTO payload(body) VALUES (?1)", [header])
        .unwrap();
    drop(database);
    let published = publish_cut(root).await;
    let canonical = fs::read(
        published
            .root
            .join("records/canonical-records/records.sqlite3"),
    )
    .unwrap();
    assert!(holds(&canonical, header));
}

#[tokio::test]
async fn an_oversized_session_is_deferred_and_the_cut_continues() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(root.join("environment")).unwrap();
    fs::write(
        root.join("environment/leases.jsonl"),
        b"{\"lease\":\"still-cut\"}\n",
    )
    .unwrap();
    let kept = b"SMALL-CONVERSATION-STAYS-NAMED";
    let kept_incarnation = [0x11u8; 16];
    let bulk_incarnation = [0x22u8; 16];
    let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let offset = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let bulk_len = usize::try_from(crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT)
        .unwrap()
        + 4096;
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    database
        .execute_batch(
            "CREATE TABLE event_streams(
                stream_key INTEGER PRIMARY KEY,
                public_id TEXT NOT NULL,
                incarnation BLOB NOT NULL,
                floor BLOB NOT NULL,
                tail BLOB NOT NULL,
                retired INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE event_stream_names(
                public_id TEXT PRIMARY KEY,
                latest_incarnation BLOB NOT NULL,
                active_stream_key INTEGER
            );
            CREATE TABLE event_records(
                stream_key INTEGER NOT NULL,
                offset BLOB NOT NULL,
                event_id TEXT NOT NULL,
                schema_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload BLOB NOT NULL
            );",
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (1, 'conv-kept', ?1, ?2, ?2)",
            [kept_incarnation.as_slice(), tail.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (2, 'conv-bulk', ?1, ?2, ?2)",
            [bulk_incarnation.as_slice(), tail.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
             VALUES ('conv-kept', ?1, 1), ('conv-bulk', ?2, 2)",
            [kept_incarnation.as_slice(), bulk_incarnation.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (1, ?1, 'kept-event', 'nessa.record', 1, ?2)",
            rusqlite_pair(&offset, kept),
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (2, ?1, 'bulk-event', 'nessa.record', 1, zeroblob(?2))",
            nessa_local_database::rusqlite::params![offset.as_slice(), bulk_len as i64],
        )
        .unwrap();
    drop(database);
    let published = publish_cut(root).await;
    let kept_digest = deferred_digest(
        DeferredHead {
            incarnation: &kept_incarnation,
            floor: &tail,
            tail: &tail,
        },
        &[DeferredRecord {
            offset: &offset,
            schema_id: "nessa.record",
            schema_version: 1,
            event_id: "kept-event",
            payload: kept,
        }],
    );
    let bulk_digest = deferred_digest_zero_payload(
        DeferredHead {
            incarnation: &bulk_incarnation,
            floor: &tail,
            tail: &tail,
        },
        DeferredRecord {
            offset: &offset,
            schema_id: "nessa.record",
            schema_version: 1,
            event_id: "bulk-event",
            payload: &[],
        },
        bulk_len,
    );
    assert!(published.text.contains(&format!(
        "deferred canonical-records conv-kept {} {kept_digest}",
        kept.len()
    )));
    assert!(published.text.contains(&format!(
        "deferred canonical-records conv-bulk {bulk_len} {bulk_digest}"
    )));
    assert!(published.text.contains(&format!(
        "deferred notes conv-kept {} {kept_digest}",
        kept.len()
    )));
    assert!(published.text.contains(&format!(
        "deferred notes conv-bulk {bulk_len} {bulk_digest}"
    )));
    assert!(published
        .text
        .contains("watermark canonical-records deferred"));
    assert!(published.text.contains("watermark notes deferred"));
    assert!(!published.text.contains("oversized"));
    assert!(holds(&published.bytes, b"still-cut"));
    assert!(!holds(&published.bytes, kept));
    assert!(!published
        .root
        .join("records/canonical-records/records.sqlite3")
        .exists());
}

fn rusqlite_pair<'a>(offset: &'a [u8], payload: &'a [u8]) -> [&'a [u8]; 2] {
    [offset, payload]
}

struct DeferredHead<'a> {
    incarnation: &'a [u8],
    floor: &'a [u8],
    tail: &'a [u8],
}

struct DeferredRecord<'a> {
    offset: &'a [u8],
    schema_id: &'a str,
    schema_version: i64,
    event_id: &'a str,
    payload: &'a [u8],
}

fn deferred_digest(head: DeferredHead<'_>, records: &[DeferredRecord<'_>]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hash_head(&mut hasher, head);
    for record in records {
        hash_record_prefix(&mut hasher, record);
        let payload_len = u64::try_from(record.payload.len()).unwrap();
        hasher.update(payload_len.to_be_bytes());
        hasher.update(record.payload);
    }
    hex_digest(hasher.finalize().as_slice())
}

fn deferred_digest_zero_payload(
    head: DeferredHead<'_>,
    record: DeferredRecord<'_>,
    payload_len: usize,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hash_head(&mut hasher, head);
    hash_record_prefix(&mut hasher, &record);
    hasher.update(u64::try_from(payload_len).unwrap().to_be_bytes());
    let chunk = [0u8; 1024 * 1024];
    let mut remaining = payload_len;
    while remaining > 0 {
        let take = remaining.min(chunk.len());
        hasher.update(&chunk[..take]);
        remaining -= take;
    }
    hex_digest(hasher.finalize().as_slice())
}

fn hash_head(hasher: &mut sha2::Sha256, head: DeferredHead<'_>) {
    use sha2::Digest;
    hasher.update(b"deferred-session/1");
    hasher.update(head.incarnation);
    hasher.update(head.floor);
    hasher.update(head.tail);
}

fn hash_record_prefix(hasher: &mut sha2::Sha256, record: &DeferredRecord<'_>) {
    use sha2::Digest;
    hasher.update(record.offset);
    write_prefixed(hasher, record.schema_id.as_bytes());
    hasher.update(record.schema_version.to_be_bytes());
    write_prefixed(hasher, record.event_id.as_bytes());
}

fn write_prefixed(hasher: &mut sha2::Sha256, bytes: &[u8]) {
    use sha2::Digest;
    let len = u32::try_from(bytes.len()).unwrap();
    hasher.update(len.to_be_bytes());
    hasher.update(bytes);
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0xf) as usize] as char);
    }
    text
}

#[tokio::test]
async fn a_deleted_secret_is_absent_from_the_session_backup() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join("records.sqlite3");
    let needle = b"DELETED-SECRET-MARKER-9f3c1a7e";
    let marker = needle.repeat(400);
    let database = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    database
        .execute_batch("PRAGMA secure_delete = OFF;")
        .unwrap();
    database
        .execute("CREATE TABLE note(body BLOB)", [])
        .unwrap();
    database
        .execute("INSERT INTO note(body) VALUES (?1)", [marker.as_slice()])
        .unwrap();
    database
        .execute(
            "INSERT INTO note(body) VALUES (?1)",
            [b"kept-note".as_slice()],
        )
        .unwrap();
    database
        .execute("DELETE FROM note WHERE body = ?1", [marker.as_slice()])
        .unwrap();
    drop(database);
    let source = fs::read(&path).unwrap();
    assert!(
        holds(&source, needle),
        "the source file must still hold the deleted secret"
    );
    let published = publish_cut(root).await;
    let canonical = fs::read(
        published
            .root
            .join("records/canonical-records/records.sqlite3"),
    )
    .unwrap();
    assert!(!holds(&canonical, needle));
    assert!(holds(&canonical, b"kept-note"));
    let copy = tempfile::NamedTempFile::new().unwrap();
    fs::write(copy.path(), &canonical).unwrap();
    let opened = nessa_local_database::rusqlite::Connection::open(copy.path()).unwrap();
    let kept: Vec<u8> = opened
        .query_row("SELECT body FROM note", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, b"kept-note");
    let freelist: i64 = opened
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .unwrap();
    assert_eq!(freelist, 0);
}

#[tokio::test]
async fn a_failed_export_releases_admission_and_publishes_nothing() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("blocked"), b"x").unwrap();
    let publisher = DirectoryPublisher::new(root.path().join("blocked").join("cut"));
    assert!(publisher.put("manifest", &[]).await.is_err());
    assert!(!root
        .path()
        .join("blocked")
        .join("cut")
        .join("manifest.txt")
        .exists());
    assert!(fs::read(root.path().join("blocked")).unwrap() == b"x");
}

#[tokio::test]
async fn concurrent_publishes_of_one_root_do_not_mix() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("cut");
    let publisher = DirectoryPublisher::new(target.clone());
    let left = vec![PublishedRecord {
        store: StoreKind::LeaseRecords,
        identity: "note".into(),
        bytes: b"left-body".to_vec(),
    }];
    let right = vec![PublishedRecord {
        store: StoreKind::LeaseRecords,
        identity: "note".into(),
        bytes: b"right-body".to_vec(),
    }];
    let (left_put, right_put) = tokio::join!(
        publisher.put("LEFT\n", &left),
        publisher.put("RIGHT\n", &right),
    );
    let succeeded = [left_put.is_ok(), right_put.is_ok()]
        .into_iter()
        .filter(|ok| *ok)
        .count();
    assert_eq!(succeeded, 1);
    let manifest = fs::read_to_string(target.join("manifest.txt")).unwrap();
    let body = fs::read(target.join("records/lease-records/note")).unwrap();
    match manifest.as_str() {
        "LEFT\n" => assert_eq!(body, b"left-body"),
        "RIGHT\n" => assert_eq!(body, b"right-body"),
        other => panic!("published manifest was {other:?}"),
    }
    assert!(publisher.put("THIRD\n", &[]).await.is_err());
    assert_eq!(
        fs::read_to_string(target.join("manifest.txt")).unwrap(),
        manifest
    );
    let leftover = fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial")
    });
    assert!(!leftover);
}

struct PublishedCut {
    manifest: crate::backup::domain::AuthenticatedManifest,
    _bundle: tempfile::TempDir,
    root: std::path::PathBuf,
    text: String,
    bytes: Vec<u8>,
}

async fn publish_cut(root: &Path) -> PublishedCut {
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let cut = bundle.path().join("cut");
    let text = fs::read_to_string(cut.join("manifest.txt")).unwrap();
    assert_eq!(text, manifest.encoded());
    let bytes = read_tree(&cut);
    PublishedCut {
        manifest,
        root: cut,
        text,
        bytes,
        _bundle: bundle,
    }
}

fn holds(bytes: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && bytes.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn a_directory_pin_does_not_read_the_body() {
    use std::io::Seek;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audit.json");
    let body = vec![b'a'; 256 * 1024];
    fs::write(&path, &body).unwrap();
    #[cfg(target_os = "linux")]
    let before = thread_bytes_read();
    let mut file = crate::backup::infrastructure::open_at_cut(&path).unwrap();
    #[cfg(target_os = "linux")]
    {
        let read = thread_bytes_read().saturating_sub(before);
        assert!(
            read < (body.len() as u64) / 2,
            "the pin read {read} bytes of a {}-byte file",
            body.len()
        );
    }
    assert_eq!(file.stream_position().unwrap(), 0);
    assert_eq!(file.metadata().unwrap().len(), body.len() as u64);
}

#[cfg(target_os = "linux")]
fn thread_bytes_read() -> u64 {
    let text = fs::read_to_string("/proc/thread-self/io").unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix("rchar: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn a_directory_rewrite_after_the_hold_is_not_the_cut() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let audit = root.join("conversations/audit/creation");
    fs::create_dir_all(&audit).unwrap();
    let name = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json";
    let path = audit.join(name);
    fs::write(&path, b"{\"cut\":\"before\"}\n").unwrap();
    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let bundle = tempfile::tempdir().unwrap();
    let manifest = capture_namespace(
        root,
        &RewriteOnRelease {
            path: path.clone(),
            bytes: b"{\"cut\":\"after\"}\n".to_vec(),
        },
        &inventory,
        &DirectoryPublisher::new(bundle.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let published = fs::read(
        bundle
            .path()
            .join("cut/records/conversation-audit")
            .join(format!("creation%2F{name}")),
    )
    .unwrap();
    assert_eq!(published, b"{\"cut\":\"before\"}\n");
    assert_eq!(fs::read(&path).unwrap(), b"{\"cut\":\"after\"}\n");
    assert!(manifest.encoded().contains("conversation-audit"));
}

struct AppendOnRelease {
    appends: Vec<(std::path::PathBuf, Vec<u8>)>,
    creates: Vec<(std::path::PathBuf, Vec<u8>)>,
}

#[async_trait]
impl MutationAdmission for AppendOnRelease {
    async fn hold(
        &self,
    ) -> Result<
        Box<dyn crate::backup::application::AdmissionLease + Send + Sync>,
        crate::backup::application::AdmissionFailure,
    > {
        Ok(Box::new(AppendHold {
            appends: self.appends.clone(),
            creates: self.creates.clone(),
        }))
    }
}

struct AppendHold {
    appends: Vec<(std::path::PathBuf, Vec<u8>)>,
    creates: Vec<(std::path::PathBuf, Vec<u8>)>,
}

impl Drop for AppendHold {
    fn drop(&mut self) {
        for (path, bytes) in &self.appends {
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            std::io::Write::write_all(&mut file, bytes).unwrap();
        }
        for (path, bytes) in &self.creates {
            fs::write(path, bytes).unwrap();
        }
    }
}

impl crate::backup::application::AdmissionLease for AppendHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

struct RewriteOnRelease {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

#[async_trait]
impl MutationAdmission for RewriteOnRelease {
    async fn hold(
        &self,
    ) -> Result<
        Box<dyn crate::backup::application::AdmissionLease + Send + Sync>,
        crate::backup::application::AdmissionFailure,
    > {
        Ok(Box::new(RewriteHold {
            path: self.path.clone(),
            bytes: self.bytes.clone(),
        }))
    }
}

struct RewriteHold {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

impl Drop for RewriteHold {
    fn drop(&mut self) {
        let next = self.path.with_extension("json.next");
        fs::write(&next, &self.bytes).unwrap();
        fs::rename(&next, &self.path).unwrap();
    }
}

impl crate::backup::application::AdmissionLease for RewriteHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

struct OpenAdmission;

#[async_trait]
impl MutationAdmission for OpenAdmission {
    async fn hold(
        &self,
    ) -> Result<
        Box<dyn crate::backup::application::AdmissionLease + Send + Sync>,
        crate::backup::application::AdmissionFailure,
    > {
        Ok(Box::new(OpenHold))
    }
}

struct OpenHold;

impl crate::backup::application::AdmissionLease for OpenHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

struct Clock(u64);

impl ExportClock for Clock {
    fn unix_ms(&self) -> u64 {
        self.0
    }
}

#[test]
fn a_sqlite_snapshot_excludes_a_later_write() {
    use std::time::Duration;

    use nessa_local_database::rusqlite::Connection;

    use crate::backup::infrastructure::snapshot::SqliteSnapshot;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("records.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE t(v INTEGER); INSERT INTO t VALUES (1);")
        .unwrap();
    drop(connection);

    let snapshot = SqliteSnapshot::open(&path).unwrap();
    let watch = TempCreateWatch::arm();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let connection = Connection::open(&writer_path).unwrap();
        connection.busy_timeout(Duration::from_secs(5)).unwrap();
        connection.execute("UPDATE t SET v = 2", []).unwrap();
    });
    let bytes = match snapshot.backup_bytes().unwrap() {
        crate::backup::infrastructure::snapshot::SnapshotCopy::Ready(bytes) => bytes,
        crate::backup::infrastructure::snapshot::SnapshotCopy::Deferred(_) => {
            panic!("the fixture is within the session export limit");
        }
    };
    watch.assert_no_export_file();
    drop(snapshot);
    writer.join().unwrap();

    let copy = directory.path().join("snapshot.sqlite3");
    fs::write(&copy, bytes).unwrap();
    let copied = Connection::open(&copy).unwrap();
    let snapshotted: i64 = copied
        .query_row("SELECT v FROM t", [], |row| row.get(0))
        .unwrap();
    assert_eq!(snapshotted, 1);
    let live = Connection::open(&path).unwrap();
    let current: i64 = live
        .query_row("SELECT v FROM t", [], |row| row.get(0))
        .unwrap();
    assert_eq!(current, 2);
}

/// Sees a `nessa-export-*` file created in the process temp directory.
///
/// On Linux the watch is inotify, so a file created and removed during
/// [`SqliteSnapshot::backup_bytes`] is still reported. Other hosts only
/// check that no such file remains.
struct TempCreateWatch {
    #[cfg(target_os = "linux")]
    fd: i32,
}

impl TempCreateWatch {
    fn arm() -> Self {
        #[cfg(target_os = "linux")]
        {
            unsafe {
                let fd = inotify_init1(0x800);
                assert!(fd >= 0, "inotify_init1");
                let directory = std::ffi::CString::new(
                    std::env::temp_dir().to_str().expect("temp dir is utf-8"),
                )
                .unwrap();
                assert!(
                    inotify_add_watch(fd, directory.as_ptr(), 0x100) >= 0,
                    "inotify_add_watch"
                );
                let mut buf = [0u8; 4096];
                while read(fd, buf.as_mut_ptr().cast(), buf.len()) > 0 {}
                Self { fd }
            }
        }
        #[cfg(not(target_os = "linux"))]
        Self {}
    }

    fn assert_no_export_file(self) {
        #[cfg(target_os = "linux")]
        {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let read_bytes = unsafe { read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
                if read_bytes < 0 {
                    assert_eq!(
                        std::io::Error::last_os_error().kind(),
                        std::io::ErrorKind::WouldBlock
                    );
                    break;
                }
                let mut offset = 0usize;
                let read_bytes = read_bytes as usize;
                while offset + 16 <= read_bytes {
                    let len = u32::from_ne_bytes(buf[offset + 12..offset + 16].try_into().unwrap())
                        as usize;
                    let end = offset + 16 + len;
                    assert!(end <= read_bytes);
                    let name = buf[offset + 16..end]
                        .split(|byte| *byte == 0)
                        .next()
                        .unwrap_or(&[]);
                    let name = String::from_utf8_lossy(name);
                    assert!(
                        !name.starts_with("nessa-export-"),
                        "snapshot wrote {name} into the shared temp directory"
                    );
                    offset = end;
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let left = std::env::temp_dir()
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("nessa-export-"))
                });
            assert!(!left);
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for TempCreateWatch {
    fn drop(&mut self) {
        unsafe {
            close(self.fd);
        }
    }
}

#[cfg(target_os = "linux")]
extern "C" {
    fn inotify_init1(flags: i32) -> i32;
    fn inotify_add_watch(fd: i32, pathname: *const i8, mask: u32) -> i32;
    fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
    fn close(fd: i32) -> i32;
}

fn read_tree(root: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        } else {
            bytes.extend(fs::read(path).unwrap());
        }
    }
    bytes
}
