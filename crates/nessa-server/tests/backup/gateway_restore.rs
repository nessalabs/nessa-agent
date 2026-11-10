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
    deliver_historical_commands, AcceptedBinding, AdmissionFailure, AdmissionLease,
    DeferredConversationOpen, DeferredOpenFailure, ExportClock, MutationAdmission,
};
use crate::backup::domain::{credentials_imported_on_restore, IDENTITY_BODY, REPLAY_BODY};
use crate::backup::infrastructure::snapshot::SESSION_EXPORT_LIMIT;
use crate::backup::infrastructure::{
    capture_namespace, CedarInventoryPolicy, DirectoryPublisher, FileDeletionLog,
};
use crate::composition::{
    bind_restored_owner, install_serving_stores, merge_published_session, run_backup_restore,
    startup_authority, GatewayDeferredOpen,
};
use crate::conversation::application::{
    ConversationCaller, ConversationCatalogue, ConversationError, ConversationLimits,
    ConversationListing, Reader, RequestedConversation,
};
use crate::conversation::infrastructure::LocalConversationStore;
use crate::core::RunError;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;

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
    open_served(&gateway, &binding, "conv-bulk").unwrap();
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
        open_served(&gateway, &binding, "conv-kept").unwrap_err(),
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

#[test]
fn a_failed_metadata_install_publishes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let metadata = live.join("records/conversation-metadata");
    fs::create_dir_all(&metadata).unwrap();
    fs::write(
        metadata.join("conversation-conv-real"),
        br#"{"id":"conv-real","organization":"acme","owner":"ada","agent":"claude","model":"opus","approvalMode":"ask","creationRevision":3,"changeRevision":4}"#,
    )
    .unwrap();
    fs::write(live.join("deferred.txt"), b"not a deferred list\n").unwrap();
    let namespace = root.path().join("gateway");
    fs::create_dir(&namespace).unwrap();
    assert!(install_serving_stores(&namespace, &live).is_err());
    assert!(!namespace.join("conversations/metadata.sqlite3").exists());
    assert!(!namespace
        .join("conversations/metadata.sqlite3.partial")
        .exists());
    fs::remove_file(live.join("deferred.txt")).unwrap();
    install_serving_stores(&namespace, &live).unwrap();
    assert!(namespace.join("conversations/metadata.sqlite3").is_file());
}

#[tokio::test]
async fn a_restored_catalogue_lists_for_the_new_local_owner() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let metadata_dir = live.join("records/conversation-metadata");
    fs::create_dir_all(&metadata_dir).unwrap();
    fs::write(
        metadata_dir.join("conversation-00000000-0000-4000-8000-000000000001"),
        br#"{"id":"00000000-0000-4000-8000-000000000001","organization":"acme","owner":"ada","agent":"claude","model":"opus","approvalMode":"ask","creationRevision":3,"changeRevision":4}"#,
    )
    .unwrap();
    fs::write(
        metadata_dir.join("summary-00000000-0000-4000-8000-000000000001"),
        br#"{"conversationId":"00000000-0000-4000-8000-000000000001","title":"Quarterly","preview":"notes","updatedAtMs":42,"archived":false}"#,
    )
    .unwrap();
    let namespace = root.path().join("gateway");
    fs::create_dir_all(namespace.join("auth")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in [root.path(), &namespace, &namespace.join("auth")] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    install_serving_stores(&namespace, &live).unwrap();
    let credentials = nessa_auth::adapters::local::LocalCredentialStore::open(
        namespace.join("auth"),
        "credentials.v1.json",
        std::sync::Arc::new(nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap()),
    )
    .unwrap();
    credentials
        .bootstrap(nessa_auth::adapters::local::BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: nessa_auth::application::dto::OrganizationInputDto {
                id: "org-new".into(),
            },
            principal: nessa_auth::application::dto::PrincipalInputDto {
                id: "person-new".into(),
                kind: nessa_auth::application::dto::PrincipalKindDto::Human,
            },
            membership: nessa_auth::application::dto::MembershipInputDto {
                id: "member".into(),
                principal_id: "person-new".into(),
                organization_id: "org-new".into(),
                role: nessa_auth::application::dto::MembershipRoleDto::Admin,
                state: nessa_auth::application::dto::MembershipStateDto::Active,
            },
            credential_id: "credential".into(),
            issued_at: 1,
            expires_at: None,
            grants: vec![nessa_auth::application::dto::CredentialGrantDto {
                action: "credential.manage".into(),
                resource: nessa_auth::application::dto::ResourceDto {
                    organization_id: "org-new".into(),
                    id: "gateway".into(),
                },
            }],
        })
        .unwrap();
    let (organization, owner) = credentials.personal_owner().unwrap();
    let catalogue = namespace.join("conversations/metadata.sqlite3");
    bind_restored_owner(&catalogue, &organization, &owner).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            namespace.join("conversations"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let store = LocalConversationStore::open(&catalogue).unwrap();
    let organization = OrganizationId::new(&organization).unwrap();
    let owner = PrincipalId::new(&owner).unwrap();
    let listed = store
        .list(&organization, &owner, false, 10, &Reader::Owner)
        .await
        .unwrap();
    let ids: Vec<_> = listed
        .conversations
        .iter()
        .map(|conversation| conversation.conversation.id().to_string())
        .collect();
    assert_eq!(
        ids,
        vec!["00000000-0000-4000-8000-000000000001".to_string()]
    );
    let previous = store
        .list(
            &OrganizationId::new("acme").unwrap(),
            &PrincipalId::new("ada").unwrap(),
            false,
            10,
            &Reader::Owner,
        )
        .await
        .unwrap();
    assert!(previous.conversations.is_empty());
}

#[test]
fn the_first_session_database_is_absent_until_its_copy_finishes() {
    let root = tempfile::tempdir().unwrap();
    let namespace = root.path().join("gateway");
    let live = root.path().join("live");
    fs::create_dir_all(live.join("sessions")).unwrap();
    let published = live.join("sessions/conv-new.sqlite3");
    write_one_stream(&published, "conv-new", b"fetched-body");
    let sessions = namespace.join("conversations/sessions");
    fs::create_dir_all(&sessions).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&sessions, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(sessions.join("records.sqlite3.partial"), b"truncated").unwrap();
    merge_published_session(&namespace, &live, "conv-new").unwrap();
    let serving = sessions.join("records.sqlite3");
    assert!(serving.is_file());
    assert!(!sessions.join("records.sqlite3.partial").exists());
    let connection = nessa_local_database::rusqlite::Connection::open(&serving).unwrap();
    let payload: Vec<u8> = connection
        .query_row("SELECT payload FROM event_records", [], |row| row.get(0))
        .unwrap();
    assert_eq!(payload, b"fetched-body");
}

#[tokio::test]
async fn a_missing_restored_source_does_not_open_a_fresh_session() {
    let (service, provider, _, _) =
        crate::conversation_test_support::fixture(ConversationLimits::default());
    service.set_open_preparation(std::sync::Arc::new(MissingRestoredSource));
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    let error = service
        .create(
            id,
            ConversationCaller {
                organization_id: OrganizationId::new("org").unwrap(),
                principal_id: PrincipalId::new("person").unwrap(),
                surface_id: "panel".into(),
                action_id: "create".into(),
            },
            RequestedConversation::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ConversationError::DeferredSourceUnavailable
    ));
    assert_eq!(
        provider
            .open_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

struct MissingRestoredSource;

impl crate::conversation::application::OpenPreparation for MissingRestoredSource {
    fn prepare(&self, _: &str) -> Result<(), ConversationError> {
        Err(ConversationError::DeferredSourceUnavailable)
    }
}

fn open_served(
    namespace: &std::path::Path,
    binding: &AcceptedBinding,
    conversation_id: &str,
) -> Result<(), DeferredOpenFailure> {
    let policy = CedarInventoryPolicy::new().map_err(|_| DeferredOpenFailure::Unavailable)?;
    GatewayDeferredOpen::new(namespace.to_path_buf(), binding.clone(), policy).open(conversation_id)
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

#[test]
fn a_summary_keeps_its_title_and_the_conversation_keeps_its_owner() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let metadata = live.join("records/conversation-metadata");
    fs::create_dir_all(&metadata).unwrap();
    // The summary is written first. Directory order must not decide which
    // record supplies the owner and which supplies the title.
    fs::write(
        metadata.join("summary-conv-real"),
        br#"{"conversationId":"conv-real","title":"Quarterly","preview":"notes","updatedAtMs":42,"archived":false}"#,
    )
    .unwrap();
    fs::write(
        metadata.join("conversation-conv-real"),
        br#"{"id":"conv-real","organization":"acme","owner":"ada","agent":"claude","model":"opus","approvalMode":"ask","creationRevision":3,"changeRevision":4}"#,
    )
    .unwrap();
    fs::write(
        metadata.join("summary-conv-title-only"),
        br#"{"conversationId":"conv-title-only","title":"Orphan","updatedAtMs":1,"archived":false}"#,
    )
    .unwrap();
    fs::write(
        metadata.join("tombstone-conv-gone"),
        br#"{"conversationId":"conv-gone","erased":true}"#,
    )
    .unwrap();
    let namespace = root.path().join("gateway");
    fs::create_dir(&namespace).unwrap();
    install_serving_stores(&namespace, &live).unwrap();
    let connection = nessa_local_database::rusqlite::Connection::open(
        namespace.join("conversations/metadata.sqlite3"),
    )
    .unwrap();
    let owner: (String, String, String, i64, i64) = connection
        .query_row(
            "SELECT organization, owner, agent, creation_revision, change_revision
             FROM conversations WHERE id = 'conv-real'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(owner, ("acme".into(), "ada".into(), "claude".into(), 3, 4));
    let summary: (Option<String>, Option<String>, i64, i64) = connection
        .query_row(
            "SELECT title, preview, updated_at_ms, archived FROM summaries WHERE conversation_id = 'conv-real'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        summary,
        (Some("Quarterly".into()), Some("notes".into()), 42, 0)
    );
    let others: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversations WHERE id IN ('conv-title-only', 'conv-gone')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(others, 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in [root.path(), &namespace, &namespace.join("conversations")] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let revision = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            let store =
                LocalConversationStore::open(&namespace.join("conversations/metadata.sqlite3"))
                    .unwrap();
            store
                .head(
                    &OrganizationId::new("acme").unwrap(),
                    &PrincipalId::new("ada").unwrap(),
                    &Reader::Owner,
                )
                .await
                .unwrap()
                .revision
        });
    assert_eq!(revision, 4);
}

#[test]
fn a_fetched_stream_is_absent_until_its_records_are_written_with_it() {
    let root = tempfile::tempdir().unwrap();
    let namespace = root.path().join("gateway");
    let live = root.path().join("live");
    let serving = namespace.join("conversations/sessions/records.sqlite3");
    fs::create_dir_all(serving.parent().unwrap()).unwrap();
    fs::create_dir_all(live.join("sessions")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(serving.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    write_session(&serving, 64);
    let incomplete = live.join("sessions/conv-new.sqlite3");
    write_stream_without_records(&incomplete, "conv-new");
    let error = merge_published_session(&namespace, &live, "conv-new").unwrap_err();
    assert!(!error.is_empty());
    let connection = nessa_local_database::rusqlite::Connection::open(&serving).unwrap();
    let stranded: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM event_streams WHERE public_id = 'conv-new'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stranded, 0);
    let kept: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM event_records WHERE stream_key = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
    drop(connection);
    fs::remove_file(&incomplete).unwrap();
    write_one_stream(&incomplete, "conv-new", b"fetched-body");
    merge_published_session(&namespace, &live, "conv-new").unwrap();
    merge_published_session(&namespace, &live, "conv-new").unwrap();
    let connection = nessa_local_database::rusqlite::Connection::open(&serving).unwrap();
    let streams: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM event_streams WHERE public_id = 'conv-new'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(streams, 1);
    let payload: Vec<u8> = connection
        .query_row(
            "SELECT payload FROM event_records
             WHERE stream_key = (
                SELECT stream_key FROM event_streams WHERE public_id = 'conv-new'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(payload, b"fetched-body");
}

fn write_stream_without_records(path: &PathBuf, conversation_id: &str) {
    let database = nessa_local_database::rusqlite::Connection::open(path).unwrap();
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
            );",
        )
        .unwrap();
    let incarnation = [0x33u8; 16];
    let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (7, ?1, ?2, ?3, ?3)",
            nessa_local_database::rusqlite::params![
                conversation_id,
                incarnation.as_slice(),
                tail.as_slice()
            ],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
             VALUES (?1, ?2, 7)",
            nessa_local_database::rusqlite::params![conversation_id, incarnation.as_slice()],
        )
        .unwrap();
}

fn write_one_stream(path: &PathBuf, conversation_id: &str, payload: &[u8]) {
    let database = nessa_local_database::rusqlite::Connection::open(path).unwrap();
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
    let incarnation = [0x33u8; 16];
    let tail = [0u8, 0, 0, 0, 0, 0, 0, 1];
    let offset = [0u8, 0, 0, 0, 0, 0, 0, 1];
    database
        .execute(
            "INSERT INTO event_streams(stream_key, public_id, incarnation, floor, tail)
             VALUES (7, ?1, ?2, ?3, ?3)",
            nessa_local_database::rusqlite::params![
                conversation_id,
                incarnation.as_slice(),
                tail.as_slice()
            ],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_stream_names(public_id, latest_incarnation, active_stream_key)
             VALUES (?1, ?2, 7)",
            nessa_local_database::rusqlite::params![conversation_id, incarnation.as_slice()],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO event_records(stream_key, offset, event_id, schema_id, schema_version, payload)
             VALUES (7, ?1, 'fetched-event', 'nessa.record', 1, ?2)",
            nessa_local_database::rusqlite::params![offset.as_slice(), payload],
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
