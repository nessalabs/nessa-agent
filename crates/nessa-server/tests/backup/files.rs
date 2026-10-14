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
    assert!(published
        .text
        .contains("redacted conversation-audit aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json"));
    assert!(published
        .text
        .contains("skipped conversation-audit aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json secret"));
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
async fn an_oversized_session_is_skipped_and_the_cut_continues() {
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
    let database =
        nessa_local_database::rusqlite::Connection::open(sessions.join("records.sqlite3")).unwrap();
    let limit =
        i64::try_from(crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT).unwrap();
    database
        .execute_batch("CREATE TABLE blob(body BLOB);")
        .unwrap();
    database
        .execute(
            "INSERT INTO blob(body) VALUES (zeroblob(?1))",
            [limit + 4096],
        )
        .unwrap();
    drop(database);
    let published = publish_cut(root).await;
    assert!(published
        .text
        .contains("skipped canonical-records records.sqlite3 oversized"));
    assert!(published
        .text
        .contains("skipped notes records.sqlite3 oversized"));
    assert!(holds(&published.bytes, b"still-cut"));
    assert!(!published
        .root
        .join("records/canonical-records/records.sqlite3")
        .exists());
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
        crate::backup::infrastructure::snapshot::SnapshotCopy::Oversized => {
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
