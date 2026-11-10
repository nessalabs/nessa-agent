//! The restore command and the gateway that serves an accepted backup.
//!
//! A staged tree is not the live path. Acceptance is what makes it the
//! authority. Opening one oversized conversation fetches that version, and a
//! source that is gone is a refusal that leaves every other session file as
//! it was. Historical outbox names are not executed.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::backup::application::{
    deliver_historical_commands, AdmissionFailure, AdmissionLease, DeferredOpenFailure,
    ExportClock, MutationAdmission,
};
use crate::backup::domain::{credentials_imported_on_restore, IDENTITY_BODY, REPLAY_BODY};
use crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT;
use crate::backup::infrastructure::{capture_namespace, DirectoryPublisher, FileDeletionLog};
use crate::composition::{open_restored_conversation, run_backup_restore, startup_authority};
use crate::core::RunError;

struct Clock(u64);

impl ExportClock for Clock {
    fn unix_ms(&self) -> u64 {
        self.0
    }
}

struct OpenAdmission;

#[async_trait]
impl MutationAdmission for OpenAdmission {
    async fn hold(&self) -> Result<Box<dyn AdmissionLease + Send + Sync>, AdmissionFailure> {
        Ok(Box::new(OpenHold))
    }
}

struct OpenHold;

impl AdmissionLease for OpenHold {
    fn retirement_signaled(&self) -> bool {
        false
    }
}

fn arg(text: impl AsRef<std::ffi::OsStr>) -> OsString {
    OsString::from(text.as_ref())
}

fn run(arguments: &[OsString]) -> (u8, String) {
    let mut output = Vec::new();
    let code = run_backup_restore(arguments, &mut output);
    let text = String::from_utf8(output).unwrap();
    (code, text)
}

fn lines(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn a_quarantined_backup_is_accepted_before_an_oversized_conversation_is_opened() {
    let namespace = tempfile::tempdir().unwrap();
    let root = namespace.path();
    let sessions = root.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::create_dir_all(root.join("environment")).unwrap();
    fs::create_dir_all(root.join("device/outbox")).unwrap();
    fs::write(
        root.join("environment/leases.jsonl"),
        b"{\"kind\":\"granted\",\"lease\":\"lease-drill\"}\n",
    )
    .unwrap();
    fs::write(
        root.join("device/outbox/command.txt"),
        b"echo should-not-run\n",
    )
    .unwrap();
    let database_path = sessions.join("records.sqlite3");
    let bulk_len = usize::try_from(SESSION_EXPORT_LIMIT).unwrap() + 4096;
    write_session(&database_path, bulk_len);

    let inventory = FileDeletionLog::open(root.join("backup")).unwrap();
    let published = tempfile::tempdir().unwrap();
    capture_namespace(
        root,
        &OpenAdmission,
        &inventory,
        &DirectoryPublisher::new(published.path().join("cut")),
        &Clock(8),
        &inventory.key(),
    )
    .await
    .unwrap();
    let bundle = published.path().join("cut");
    let key_file = published.path().join("mac.key");
    fs::write(&key_file, inventory.key().0).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&key_file, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(published.path(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&bundle, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let destination_root = tempfile::tempdir().unwrap();
    let gateway_root = tempfile::tempdir().unwrap();
    let destination = destination_root.path().join("tree");
    let gateway = gateway_root.path().join("gateway");
    fs::create_dir(&gateway).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gateway, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (code, text) = run(&[
        arg("stage"),
        arg("--bundle"),
        bundle.as_os_str().to_os_string(),
        arg("--mac-key-file"),
        key_file.as_os_str().to_os_string(),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
        arg("--source"),
        database_path.as_os_str().to_os_string(),
    ]);
    assert_eq!(code, 0, "{text}");
    let staged = lines(&text);
    assert_eq!(staged[0]["status"], "quarantined");
    assert!(staged[0]["leases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|lease| lease == "lease-drill"));
    assert!(!destination.join("live").exists());
    let quarantined = startup_authority(&destination.join("quarantine")).unwrap_err();
    assert!(
        matches!(quarantined, RunError::Restore(ref message) if message.contains("not been accepted")),
        "{quarantined}"
    );
    assert_eq!(
        startup_authority(&gateway).unwrap(),
        None,
        "a gateway with no accepted pointer still starts as itself"
    );

    let (code, text) = run(&[
        arg("accept"),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
    ]);
    assert_eq!(code, 1, "{text}");
    assert_eq!(lines(&text)[0]["reason"], "executors-unfenced");
    assert!(!destination.join("live").exists());
    let pointer = gateway.join("backup/accepted-restore.txt");
    fs::create_dir_all(pointer.parent().unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(pointer.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(
        &pointer,
        format!(
            "nessa-accepted-restore/1\nlive {}\naudit {}\nsource -\n",
            destination.join("quarantine").display(),
            destination.join("audit").display(),
        ),
    )
    .unwrap();
    let refused = startup_authority(&gateway).unwrap_err();
    assert!(
        matches!(refused, RunError::Restore(ref message) if message.contains("not been accepted")),
        "{refused}"
    );

    let (code, text) = run(&[
        arg("accept"),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
        arg("--namespace"),
        gateway.as_os_str().to_os_string(),
        arg("--receipt"),
        arg("lease-drill=stopped-host-1"),
    ]);
    assert_eq!(code, 0, "{text}");
    assert_eq!(lines(&text)[0]["status"], "accepted");
    let (code, text) = run(&[
        arg("accept"),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
        arg("--namespace"),
        gateway.as_os_str().to_os_string(),
        arg("--receipt"),
        arg("lease-drill=stopped-host-1"),
    ]);
    assert_eq!(code, 0, "{text}");
    assert_eq!(lines(&text)[0]["status"], "accepted");
    let live = destination.join("live");
    assert_eq!(
        fs::read(live.join("identity.txt")).unwrap(),
        IDENTITY_BODY.as_bytes()
    );
    assert_eq!(
        fs::read(live.join("replay.blocked")).unwrap(),
        REPLAY_BODY.as_bytes()
    );
    let imported =
        credentials_imported_on_restore(IDENTITY_BODY.as_bytes(), &[b"secret".to_vec()]).unwrap();
    assert!(imported.is_empty());
    let commands = outbox_names(&live);
    assert!(
        commands.iter().any(|name| name.contains("command.txt")),
        "{commands:?}"
    );
    let mut ran = Vec::new();
    deliver_historical_commands(
        &fs::read(live.join("replay.blocked")).unwrap(),
        &commands,
        &mut |command| {
            ran.push(command.to_owned());
        },
    )
    .unwrap();
    assert!(ran.is_empty());
    let binding = startup_authority(&gateway).unwrap().unwrap();
    assert_eq!(binding.live, live);
    let metadata = nessa_local_database::rusqlite::Connection::open(
        gateway.join("conversations/metadata.sqlite3"),
    )
    .unwrap();
    let listed_in_service: i64 = metadata
        .query_row(
            "SELECT COUNT(*) FROM conversations WHERE id IN ('conv-bulk', 'conv-kept')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(listed_in_service, 2);

    let (code, text) = run(&[
        arg("list"),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
    ]);
    assert_eq!(code, 0, "{text}");
    let listed = lines(&text);
    let conversations = listed[0]["conversations"].as_array().unwrap();
    assert!(conversations.iter().any(|id| id == "conv-bulk"));
    assert!(conversations.iter().any(|id| id == "conv-kept"));

    let (code, text) = run(&[
        arg("open"),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
        arg("--conversation"),
        arg("conv-bulk"),
    ]);
    assert_eq!(code, 0, "{text}");
    let opened = lines(&text);
    let final_line = opened.last().unwrap();
    assert_eq!(final_line["status"], "opened");
    assert_eq!(final_line["size"], bulk_len as u64);
    assert!(opened.iter().any(|line| line["status"] == "progress"));
    let session = live.join("sessions/conv-bulk.sqlite3");
    assert!(session.is_file());
    open_restored_conversation(&gateway, &binding, "conv-bulk").unwrap();
    let serving = nessa_local_database::rusqlite::Connection::open(
        gateway.join("conversations/sessions/records.sqlite3"),
    )
    .unwrap();
    let served: i64 = serving
        .query_row(
            "SELECT COUNT(*) FROM event_streams WHERE public_id = 'conv-bulk'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(served, 1);

    fs::remove_file(&database_path).unwrap();
    let (code, text) = run(&[
        arg("open"),
        arg("--destination"),
        destination.as_os_str().to_os_string(),
        arg("--conversation"),
        arg("conv-kept"),
    ]);
    assert_eq!(code, 1, "{text}");
    assert_eq!(lines(&text).last().unwrap()["reason"], "source-missing");
    assert_eq!(
        open_restored_conversation(&gateway, &binding, "conv-kept").unwrap_err(),
        DeferredOpenFailure::SourceMissing
    );
    assert!(!live.join("sessions/conv-kept.sqlite3").exists());
    assert!(
        !gateway
            .join("conversations/sessions/records.sqlite3")
            .exists()
            || serving_lacks(&gateway, "conv-kept")
    );
    assert!(session.is_file());
}

fn serving_lacks(namespace: &std::path::Path, conversation_id: &str) -> bool {
    let Ok(connection) = nessa_local_database::rusqlite::Connection::open(
        namespace.join("conversations/sessions/records.sqlite3"),
    ) else {
        return true;
    };
    connection
        .query_row(
            "SELECT COUNT(*) FROM event_streams WHERE public_id = ?1",
            [conversation_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        == 0
}

fn outbox_names(live: &std::path::Path) -> Vec<String> {
    let mut names = Vec::new();
    for store in ["device-outbox", "environment-outbox"] {
        let Ok(entries) = fs::read_dir(live.join("records").join(store)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_file() {
                names.push(format!(
                    "records/{store}/{}",
                    entry.file_name().to_string_lossy()
                ));
            }
        }
    }
    names
}

fn write_session(database_path: &PathBuf, bulk_len: usize) {
    let kept = b"SMALL-CONVERSATION-STAYS-NAMED";
    let kept_incarnation = [0x11u8; 16];
    let bulk_incarnation = [0x22u8; 16];
    let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let offset = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let database = nessa_local_database::rusqlite::Connection::open(database_path).unwrap();
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
            [offset.as_slice(), kept.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (2, ?1, 'bulk-event', 'nessa.record', 1, zeroblob(?2))",
            nessa_local_database::rusqlite::params![offset.as_slice(), bulk_len as i64],
        )
        .unwrap();
}

#[tokio::test]
async fn the_restore_command_does_not_touch_a_profile_when_the_arguments_are_refused() {
    let canary = tempfile::tempdir().unwrap();
    let marker = canary.path().join("do-not-touch");
    fs::write(&marker, b"canary").unwrap();
    let mut output = Vec::new();
    let code = run_backup_restore(&[OsString::from("stage")], &mut output);
    assert_eq!(code, 2);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()["status"],
        "usage"
    );
    let mut help = Vec::new();
    let code = run_backup_restore(&[OsString::from("--help")], &mut help);
    assert_eq!(code, 0);
    assert!(String::from_utf8(help)
        .unwrap()
        .contains("nessa-backup-restore"));
    assert_eq!(fs::read(&marker).unwrap(), b"canary");
}
