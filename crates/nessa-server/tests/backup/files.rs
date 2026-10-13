//! Files the cut reads: a bad line is named, a secret is not copied, and a
//! local outbox is not a store.

use std::fs;
use std::path::Path;

use async_trait::async_trait;

use crate::backup::application::{ExportClock, ManifestPublisher, MutationAdmission};
use crate::backup::domain::{Sensitivity, SkipReason, StoreKind};
use crate::backup::infrastructure::{
    capture_namespace, jsonl_cut, path_sensitivity, DirectoryPublisher, FileDeletionLog,
};

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
    assert!(!bundle_bytes
        .windows(marker.len())
        .any(|window| window == marker));
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
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        let connection = Connection::open(&writer_path).unwrap();
        connection.busy_timeout(Duration::from_secs(5)).unwrap();
        connection.execute("UPDATE t SET v = 2", []).unwrap();
    });
    let bytes = snapshot.backup_bytes().unwrap();
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
