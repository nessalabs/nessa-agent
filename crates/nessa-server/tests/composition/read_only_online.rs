//! Real separate client/gateway proofs consume canonical stores and actual paired credentials.
#[path = "read_only_online/fixtures.rs"]
mod fixtures;
use fixtures::*;
use nessa_client_core::composition;
use nessa_local_database::rusqlite::Connection;
use nessa_protocol::product::generated::{
    product_event, product_method, ConversationCloseParams, ConversationRecordsHeadParams,
    ConversationRecordsHeadResult, ConversationRecordsPageParams, ConversationUnwatchParams,
    ConversationWatchRecordsParams, CredentialListParams, RecordPageRequest,
    MAX_CHANGE_WATCH_ID_BYTES, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES,
    MAX_RECORD_PAGE_RECORDS,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[test]
fn online_process_restarts_reuse_actual_binding() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache_root = directory.path().join("cache");
    nessa_local_storage::create_directory(&cache_root).unwrap();
    let profile = profile_for(&root, &cache_root.join("cache.sqlite3"));
    let args = |name: &str, target: Option<&str>, pages: Option<&str>| {
        let mut args = vec![name.into(), profile.clone()];
        if let Some(target) = target {
            args.push(target.into());
        }
        if let Some(pages) = pages {
            args.push(pages.into());
        }
        args
    };
    let (ok, empty) = command(args("check-records", Some(&setup.empty), None), false);
    assert!(ok, "{empty:?}");
    let empty = empty.unwrap();
    assert_eq!(empty["durable"]["progress"]["downloaded"], "0");
    assert_eq!(empty["durable"]["status"], "complete_empty");
    let (ok, pending) = command(
        args("sync-records", Some(&setup.conversation), Some("1")),
        false,
    );
    assert!(ok, "{pending:?}");
    let pending = pending.unwrap();
    assert_eq!(pending["work"]["complete"], false);
    assert_eq!(pending["durable"]["progress"]["downloaded"], "1");
    assert!(
        pending["capturedCheck"]["head"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 1
    );
    assert_eq!(pending["durable"]["progress"]["applied"], "0");
    assert_eq!(pending["durable"]["status"], "stale");
    let (ok, catalogue) = command(args("sync-catalogue", None, Some("10")), false);
    assert!(ok, "{catalogue:?}");
    assert_eq!(catalogue.unwrap()["work"]["complete"], true);
    let profile_bytes = std::fs::read(root.join("profile.json")).unwrap();
    let credentials = std::fs::read(root.join("credentials.json")).unwrap();
    let bindings = std::fs::read(root.join("receivers.sqlite3")).unwrap();
    drop(gateway);
    let _restarted = Gateway::start(&root);
    assert_eq!(
        std::fs::read(root.join("profile.json")).unwrap(),
        profile_bytes
    );
    assert_eq!(
        std::fs::read(root.join("credentials.json")).unwrap(),
        credentials
    );
    assert_eq!(
        std::fs::read(root.join("receivers.sqlite3")).unwrap(),
        bindings
    );
    let (ok, _) = command(
        args("sync-records", Some(&setup.conversation), Some("100")),
        true,
    );
    assert!(!ok);
    let (ok, finished) = command(
        args("check-records", Some(&setup.conversation), None),
        false,
    );
    assert!(ok, "{finished:?}");
    let finished = finished.unwrap();
    assert_eq!(finished["work"]["complete"], true);
    assert_eq!(
        finished["durable"]["progress"]["downloaded"],
        finished["durable"]["progress"]["applied"]
    );
    assert_eq!(finished["durable"]["progress"]["facts"], "1");
    assert_eq!(finished["durable"]["status"], "complete");
}
#[test]
fn online_profile_refusal_precedes_network_and_cache() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    let cache = private.join("never-created.sqlite3");
    let (ok, report) = command(
        vec![
            "check-records".into(),
            directory
                .path()
                .join("missing.json")
                .to_string_lossy()
                .into_owned(),
            uuid(),
        ],
        false,
    );
    assert!(!ok);
    assert_eq!(report.unwrap()["connectionCheck"], "notPerformed");
    assert!(!cache.exists());
}

fn record_command(root: &Path, cache: &Path, setup: &Setup, pages: &str) -> Vec<String> {
    vec![
        "sync-records".into(),
        profile_for(root, cache),
        setup.conversation.clone(),
        pages.into(),
    ]
}
fn setup_cache(directory: &Path) -> PathBuf {
    let private = directory.join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    private.join("cache.sqlite3")
}
#[test]
fn online_revocation_before_page_admission_has_no_page_effects() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, "before-page");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let (ok, report) = command(record_command(&root, &cache, &setup, "1"), false);
    assert!(!ok);
    let report = report.unwrap();
    assert!(
        report["capturedCheck"]["head"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    assert_eq!(report["durable"]["progress"], Value::Null);
    assert_eq!(report["transportFailure"]["productCode"], "unauthorized");
    assert!(report["driverFailure"].is_object());
    let database = Connection::open(&cache).unwrap();
    for table in [
        "transcript_records",
        "transcript_progress",
        "transcript_checkpoints",
    ] {
        let count: i64 = database
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}
#[test]
fn online_connection_loss_preserves_confirmed_pages() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start_mode(&root, "disconnect");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let (ok, report) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    let report = report.unwrap();
    assert!(report["transportFailure"].is_object());
    assert!(report["capturedCheck"].is_object());
    assert_eq!(report["durable"]["progress"]["downloaded"], "1");
    assert_eq!(report["durable"]["progress"]["applied"], "0");
    assert_eq!(report["durable"]["status"], "unknown");
    drop(gateway);
    let _gateway = Gateway::start(&root);
    let (ok, report) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(ok, "{report:?}");
    assert_eq!(report.unwrap()["durable"]["status"], "complete");
}

#[test]
fn online_scope_change_preserves_saved_cache() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let (ok, baseline) = command(record_command(&root, &cache, &setup, "1"), false);
    assert!(ok, "{baseline:?}");
    let before = std::fs::read(&cache).unwrap();
    drop(gateway);
    let _gateway = Gateway::start_mode(&root, "regrant");
    let (ok, report) = command(record_command(&root, &cache, &setup, "1"), false);
    assert!(!ok);
    let report = report.unwrap();
    assert_eq!(report["cacheRefusal"]["code"], "scope");
    assert_eq!(
        report["durable"]["progress"]["downloaded"],
        baseline.unwrap()["durable"]["progress"]["downloaded"]
    );
    assert_eq!(std::fs::read(cache).unwrap(), before);
}

#[test]
fn online_source_append_does_not_retarget_captured_pass() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, "append");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let (ok, report) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(ok, "{report:?}");
    let report = report.unwrap();
    assert_eq!(report["work"]["complete"], true);
    assert_eq!(
        report["capturedCheck"]["head"],
        report["durable"]["progress"]["downloaded"]
    );
    assert_eq!(report["durable"]["status"], "complete");
    let (ok, next) = command(record_command(&root, &cache, &setup, "0"), false);
    assert!(ok, "{next:?}");
    let next = next.unwrap();
    let old = report["capturedCheck"]["head"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let current = next["capturedCheck"]["head"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(current > old);
    assert_eq!(
        next["durable"]["progress"]["downloaded"],
        report["durable"]["progress"]["downloaded"]
    );
    assert_eq!(next["durable"]["status"], "stale");
}

#[test]
fn online_revocation_after_admission_preserves_confirmed_page() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, "epoch");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let (ok, report) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    let report = report.unwrap();
    assert_eq!(report["transportFailure"]["productCode"], "unauthorized");
    assert_eq!(report["durable"]["progress"]["downloaded"], "1");
    assert_eq!(report["durable"]["progress"]["applied"], "0");
    assert_eq!(report["durable"]["status"], "unknown");
    assert!(report["capturedCheck"].is_object());
    // Row PC5: the refusal asked the pinned status again. The receiver no
    // longer holds the pairing, so the gateway refuses the status: that is
    // not an authenticated Terminal, and nothing is purged.
    assert_eq!(
        report["recheck"]["enrollmentFailure"]["code"], "refused",
        "{report}"
    );
    assert!(report["recheck"]["purge"].is_null());
    let before = std::fs::read(&cache).unwrap();
    let (ok, report) = command(record_command(&root, &cache, &setup, "1"), false);
    assert!(!ok);
    let report = report.unwrap();
    assert_eq!(report["enrollmentFailure"]["code"], "refused", "{report}");
    assert!(report["purge"].is_null());
    assert_eq!(std::fs::read(cache).unwrap(), before);
}

#[test]
fn online_product_aggregate_boundary_and_read_privilege_are_actual() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let mut client = WireClient::connect(&root);
    let head = client.call(
        product_method::CONVERSATION_RECORDS_HEAD,
        &ConversationRecordsHeadParams {
            conversation_id: setup.conversation.clone(),
            receiver_id: setup.receiver.clone(),
            access_epoch: setup.epoch.to_string(),
        },
    );
    assert_eq!(head["ok"], true, "{head}");
    let head: ConversationRecordsHeadResult =
        serde_json::from_value(head["payload"].clone()).unwrap();
    let mut params = ConversationRecordsPageParams {
        conversation_id: setup.conversation.clone(),
        access_epoch: setup.epoch.to_string(),
        request: RecordPageRequest {
            scope: head.scope,
            after: "0".into(),
            target: head.head,
            max_records: MAX_RECORD_PAGE_RECORDS as u64,
            max_payload_bytes: MAX_RECORD_PAGE_PAYLOAD_BYTES as u64,
            max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES as u64,
        },
    };
    let accepted = client.call(product_method::CONVERSATION_RECORDS_PAGE, &params);
    assert_eq!(accepted["ok"], true, "{accepted}");
    assert_eq!(accepted["payload"]["records"].as_array().unwrap().len(), 1);
    params.request.max_payload_bytes += 1;
    let refused = client.call(product_method::CONVERSATION_RECORDS_PAGE, &params);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error"]["code"], "invalid_request");
    let denied = client.call(product_method::CREDENTIAL_LIST, &CredentialListParams {});
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(denied["error"]["code"], "forbidden");
    let denied = client.call(
        product_method::CONVERSATION_CLOSE,
        &ConversationCloseParams {
            conversation_id: setup.conversation,
            request_id: uuid(),
        },
    );
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(denied["error"]["code"], "forbidden");
}

/// Rows PC1, PC4: a device with no issued credential, or whose pinned status
/// cannot be read, reads nothing and opens no cache.
#[test]
fn online_unusable_enrollment_does_not_open_cache() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let original: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let private = directory.path().join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    for (index, field, value, connection, failure) in [
        (
            0,
            "stateDirectory",
            json!("unpaired"),
            "notPerformed",
            "notPaired",
        ),
        (
            1,
            "gatewayAddress",
            json!(closed.to_string()),
            "attempted",
            "io",
        ),
    ] {
        let cache = private.join(format!("absent-{index}.sqlite3"));
        let mut profile = original.clone();
        profile[field] = value;
        profile["cache"] = json!(cache);
        let path = root.join(format!("wrong-{index}.json"));
        private_write(&path, &serde_json::to_vec(&profile).unwrap());
        let path = path.to_string_lossy().into_owned();
        // Row W1: `watch` is refused at the same point, the same way.
        for args in [
            vec![
                "check-records".into(),
                path.clone(),
                setup.conversation.clone(),
            ],
            vec![
                "watch".into(),
                path.clone(),
                setup.conversation.clone(),
                "1".into(),
                "1".into(),
            ],
        ] {
            let (ok, report) = command(args, false);
            assert!(!ok);
            assert!(!cache.exists());
            let report = report.unwrap();
            assert_eq!(report["connectionCheck"], connection, "{report}");
            let code = if index == 0 {
                &report["configurationFailure"]
            } else {
                &report["enrollmentFailure"]["code"]
            };
            assert_eq!(code, failure, "{report}");
        }
    }
}

/// Rows PC3, A9 (client side): after the owner revokes the device, its next
/// command reads the authenticated Terminal status and purges the receiver's
/// cached rows with one receipt before its enrollment record goes; the
/// receiver stays fenced.
#[test]
fn online_terminal_status_purges_with_one_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let (ok, synced) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(ok, "{synced:?}");
    assert_eq!(synced.unwrap()["enrollment"]["phase"], "active");
    let catalogue = vec![
        "sync-catalogue".into(),
        profile_for(&root, &cache),
        "10".into(),
    ];
    let (ok, report) = command(catalogue.clone(), false);
    assert!(ok, "{report:?}");
    drop(gateway);
    let _gateway = Gateway::start_mode(&root, "revoke");
    let (ok, report) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    let report = report.unwrap();
    assert_eq!(report["enrollment"]["phase"], "terminal", "{report}");
    assert_eq!(report["enrollment"]["cause"], "credentialRevoked");
    let purge = &report["purge"];
    assert_eq!(purge["receiver"], setup.receiver);
    assert_eq!(purge["cause"], "terminalEnrollment");
    assert_eq!(purge["initiator"], "gatewayStatus");
    assert_eq!(purge["transcripts"], "1");
    assert_ne!(purge["records"], "0");
    assert_eq!(purge["catalogueEntries"], "2");
    let database = Connection::open(&cache).unwrap();
    for table in [
        "transcript_records",
        "transcript_progress",
        "transcript_checkpoints",
        "catalogue_entries",
        "catalogue_progress",
    ] {
        let count: i64 = database
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE receiver = ?1"),
                [&setup.receiver],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    // The ended enrollment's record went after the purge: the device is no
    // longer paired, and the purged receiver stays fenced in the cache.
    let (ok, again) = command(catalogue, false);
    assert!(!ok);
    assert_eq!(again.unwrap()["configurationFailure"], "notPaired");
    let fenced: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM cache_purges WHERE receiver = ?1",
            [&setup.receiver],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fenced, 1);
}
#[test]
fn online_saved_projection_uses_durable_progress() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let profile = profile_for(&root, &cache);
    let args = |command: &str| {
        let mut args = vec![command.into(), profile.clone(), setup.conversation.clone()];
        if command == "sync-records" {
            args.push("100".into());
        }
        args
    };
    let mut output = vec![];
    composition::execute(&args("sync-records"), &mut std::io::empty(), &mut output).unwrap();
    let sync: Value = serde_json::from_slice(&output).unwrap();
    let mut output = vec![];
    composition::execute(&args("check-records"), &mut std::io::empty(), &mut output).unwrap();
    let check: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(check["durable"]["progress"], sync["durable"]["progress"]);
    assert_eq!(check["durable"]["status"], "complete");
    assert_eq!(check["work"]["downloaded"], sync["work"]["downloaded"]);
    assert_eq!(check["work"]["pages"], 0);
    assert_eq!(
        check["capturedCheck"]["head"],
        sync["capturedCheck"]["head"]
    );
}

fn default_passive_budget(mode: &str) -> (bool, Value, Duration) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, mode);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache_root = directory.path().join("cache");
    nessa_local_storage::create_directory(&cache_root).unwrap();
    let start = Instant::now();
    let (ok, value) = command(
        vec![
            "sync-records".into(),
            profile_for(&root, &cache_root.join("cache.sqlite3")),
            setup.conversation,
            "1".into(),
        ],
        false,
    );
    (ok, value.unwrap(), start.elapsed())
}
#[test]
fn default_budget_consumes_valid_delayed_source() {
    let (ok, value, elapsed) = default_passive_budget("delayed-head");
    assert!(ok, "{value:?}");
    assert!(elapsed >= Duration::from_secs(6));
    assert!(value["transportFailure"].is_null());
    assert_eq!(value["work"]["pages"], 1);
    assert_eq!(value["durable"]["progress"]["downloaded"], "1");
}
#[test]
fn default_budget_preserves_real_server_read_timeout() {
    let (ok, value, elapsed) = default_passive_budget("timeout-head");
    assert!(!ok);
    assert!(elapsed >= Duration::from_secs(10));
    assert_eq!(value["transportFailure"]["code"], "record", "{value:?}");
    assert_eq!(value["transportFailure"]["productCode"], "read_timeout");
    assert_eq!(value["discoveryFailure"], true);
}
#[test]
fn default_budget_consumes_cumulative_valid_rpcs() {
    let (ok, value, elapsed) = default_passive_budget("cumulative-head");
    assert!(ok, "{value:?}");
    // The discovery is immediate. Authorize/head/authorize are each 3s, followed by a 7s page, in
    // the same actual driver callback, whose absolute deadline is not reset.
    assert!(elapsed >= Duration::from_secs(16));
    assert!(value["transportFailure"].is_null());
    assert_eq!(value["work"]["pages"], 1);
    assert_eq!(value["durable"]["progress"]["downloaded"], "1");
}

/// The finish line of #298 across real processes: a seeded receiver replays,
/// registers before its final recheck, catches a commit made between the
/// acknowledgement and that recheck, follows live hints, recovers a commit
/// made while it was disconnected (so its hint was never sent) from its durable
/// checkpoint, and ends with the same folded view a fresh
/// full replay produces. Revocation stops hints and the next command is refused;
/// a sleeping gateway leaves the saved view readable and the check failed.
/// Rows L1–L7 of the delivery table in docs/design/committed-change-watches.md.
#[test]
fn online_replay_then_live_hints_converge_with_a_fresh_replay() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let mut gateway = Gateway::start_live(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let facts = |report: &Value| report["durable"]["progress"]["facts"].clone();
    let sync = |cache: &Path| {
        let (ok, report) = command(record_command(&root, cache, &setup, "100"), false);
        assert!(ok, "{report:?}");
        let report = report.unwrap();
        assert_eq!(report["work"]["complete"], true, "{report}");
        assert_eq!(report["durable"]["status"], "complete", "{report}");
        report
    };
    let watch_params = ConversationWatchRecordsParams {
        conversation_id: setup.conversation.clone(),
        receiver_id: setup.receiver.clone(),
        access_epoch: setup.epoch.to_string(),
    };
    let hint = |receiver: &mut WireClient, watch: &str| match receiver.frame() {
        Frame::Text { value, bytes } => {
            assert_eq!(
                value["event"],
                product_event::CONVERSATION_CHANGED,
                "{value}"
            );
            // The hint names the watch and nothing else: no head, cursor or content.
            assert_eq!(value["payload"], json!({ "watchId": watch }));
            bytes
        }
        Frame::Closed => panic!("closed instead of a hint"),
    };

    // L1: seeded replay to a durable checkpoint.
    assert_eq!(facts(&sync(&cache)), "1");

    // L2: register; a commit lands after the acknowledgement and before the
    // final recheck (the install-to-acknowledgement window is the in-process
    // row O2). The recheck
    // from the checkpoint catches it, and the hint for it still arrives,
    // because registration came first.
    let mut receiver = WireClient::connect(&root);
    let watched = receiver.call(product_method::CONVERSATION_WATCH_RECORDS, &watch_params);
    assert_eq!(watched["ok"], true, "{watched}");
    let first_watch = watched["payload"]["watchId"].as_str().unwrap().to_owned();
    gateway.act("commit");
    assert_eq!(facts(&sync(&cache)), "2");
    let bytes = hint(&mut receiver, &first_watch);
    // Real encoded size of one hint frame; bounded by the published watch-id
    // bound plus the fixed event envelope.
    assert!(
        bytes <= MAX_CHANGE_WATCH_ID_BYTES + 128,
        "{bytes} encoded hint bytes"
    );

    // L3: a later commit is hinted, and the hint is answered by a pass.
    gateway.act("commit");
    hint(&mut receiver, &first_watch);
    assert_eq!(facts(&sync(&cache)), "3");

    // L4: the receiver disconnects and the gateway commits meanwhile, so no
    // hint for that commit is ever sent. A new connection gets a new watch identity, the old one is
    // foreign to it, and the recheck from the durable checkpoint recovers.
    drop(receiver);
    gateway.act("commit");
    let mut receiver = WireClient::connect(&root);
    let watched = receiver.call(product_method::CONVERSATION_WATCH_RECORDS, &watch_params);
    assert_eq!(watched["ok"], true, "{watched}");
    let second_watch = watched["payload"]["watchId"].as_str().unwrap().to_owned();
    assert_ne!(second_watch, first_watch);
    // Same counter as the new watch, other connection's namespace: foreign.
    let stale = receiver.call(
        product_method::CONVERSATION_UNWATCH,
        &ConversationUnwatchParams {
            watch_id: first_watch.clone(),
        },
    );
    assert_eq!(stale["ok"], false, "{stale}");
    assert_eq!(stale["error"]["code"], "invalid_watch", "{stale}");
    assert_eq!(facts(&sync(&cache)), "4");
    gateway.act("commit");
    hint(&mut receiver, &second_watch);
    assert_eq!(facts(&sync(&cache)), "5");

    // L5: replay plus live passes give the same folded view as one fresh replay.
    let show = |cache: &Path| {
        let (ok, view) = command(
            vec![
                "show".into(),
                cache.to_string_lossy().into_owned(),
                setup.receiver.clone(),
                setup.gateway.clone(),
                setup.conversation.clone(),
            ],
            false,
        );
        assert!(ok, "{view:?}");
        let mut view = view.unwrap();
        // Each `show` names its own rendering with a fresh revision; the
        // folded content and progress are what must agree.
        let revision = view["view"]["revision"].take();
        assert!(revision.is_string(), "{view}");
        view
    };
    let fresh_directory = tempfile::tempdir().unwrap();
    let fresh = setup_cache(fresh_directory.path());
    assert_eq!(facts(&sync(&fresh)), "5");
    let live_view = show(&cache);
    assert!(live_view["view"].is_object(), "{live_view}");
    assert_eq!(live_view, show(&fresh));

    // L6: access revoked mid-watch. The next commit's hint is refused at
    // delivery: the connection closes with no hint (a native close carries no
    // reason), and the next command is refused before it reads.
    gateway.act("revoke");
    gateway.act("commit");
    match receiver.frame() {
        Frame::Closed => {}
        Frame::Text { value, .. } => panic!("frame after revocation: {value}"),
    }
    let (ok, denied) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    // The receiver no longer holds the pairing, so the native profile's
    // pinned status check before any read is refused (row PC5): not an
    // authenticated Terminal, so nothing is purged.
    let denied = denied.unwrap();
    assert_eq!(denied["connectionCheck"], "attempted", "{denied}");
    assert_eq!(denied["enrollmentFailure"]["code"], "refused", "{denied}");
    assert!(denied["purge"].is_null(), "{denied}");

    // L7: the gateway sleeps. The check fails explicitly, and the saved view
    // is still read offline, unchanged.
    drop(gateway);
    let (ok, asleep) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    let asleep = asleep.unwrap();
    // The native profile has no endpoint file to find missing: the connection
    // is attempted and its pinned status check fails on I/O.
    assert_eq!(asleep["connectionCheck"], "attempted", "{asleep}");
    assert_eq!(asleep["enrollmentFailure"]["code"], "io", "{asleep}");
    assert_eq!(show(&cache), live_view);
}

/// The next line of a watch, which must be of `kind`, and when it arrived.
fn watch_line(child: &mut WatchChild, kind: &str) -> (Value, Instant) {
    let (line, at) = child.next_line();
    assert_eq!(line["kind"], kind, "{line}");
    (line, at)
}
/// A successful complete pass with this trigger, holding `facts` facts, and
/// when it arrived.
fn watch_pass(child: &mut WatchChild, trigger: &str, facts: u64) -> Instant {
    watch_pass_report(child, trigger, facts).1
}
/// As `watch_pass`, with the pass's report. Attempts the gateway answered
/// `source_preparing` before it (row W17: a slow cold read) are skipped; the
/// pass that follows them carries `trigger: preparing`.
fn watch_pass_report(child: &mut WatchChild, trigger: &str, facts: u64) -> (Value, Instant) {
    let (mut line, mut at) = watch_line(child, "pass");
    let mut trigger = trigger;
    while line["report"]["transportFailure"]["productCode"] == "source_preparing" {
        assert_eq!(line["report"]["successful"], false, "{line}");
        (line, at) = watch_line(child, "pass");
        trigger = "preparing";
    }
    assert_eq!(line["trigger"], trigger, "{line}");
    assert_eq!(line["report"]["successful"], true, "{line}");
    assert_eq!(line["report"]["work"]["complete"], true, "{line}");
    assert_eq!(
        line["report"]["durable"]["progress"]["facts"],
        facts.to_string(),
        "{line}"
    );
    (line["report"].clone(), at)
}
fn watch_hint(child: &mut WatchChild, watch: &Value, during_pass: bool) {
    let (line, _) = watch_line(child, "hint");
    assert_eq!(line["watchId"], *watch, "{line}");
    assert_eq!(line["duringPass"], during_pass, "{line}");
}
/// Registration, and the watch identity it names.
fn watch_registered(child: &mut WatchChild) -> Value {
    let (line, _) = watch_line(child, "registered");
    assert_eq!(line["enrollment"]["phase"], "active", "{line}");
    assert!(line["watchId"].is_string(), "{line}");
    line["watchId"].clone()
}
fn watch_end(mut child: WatchChild, reason: &str) -> bool {
    let (line, _) = watch_line(&mut child, "ended");
    assert_eq!(line["reason"], reason, "{line}");
    assert!(line["recheck"].is_null(), "{line}");
    child.succeeded()
}
/// Nearest-rank percentile of sorted samples.
fn percentile(sorted: &[f64], percent: usize) -> f64 {
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// Row W16: `watch`'s discovery and cache-open refusals come before
/// `registered` and are the objects `sync-records` writes for them: one line,
/// no `kind`, no `ended`; exit 1.
#[test]
fn online_watch_precondition_refusals_use_the_records_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let profile = profile_for(&root, &cache);
    let run = |command_name: &str, conversation: &str| {
        let mut args = vec![
            command_name.to_owned(),
            profile.clone(),
            conversation.to_owned(),
            "100".to_owned(),
        ];
        if command_name == "watch" {
            args.push("2".into());
        }
        let (lines, ok) = WatchChild::spawn(args).finish();
        assert!(!ok, "{lines:?}");
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines.into_iter().next().unwrap()
    };
    // Discovery: a conversation this receiver cannot read. Nothing is cached.
    let unknown = uuid();
    let refused = run("watch", &unknown);
    assert_eq!(refused["discoveryFailure"], true, "{refused}");
    assert!(refused.get("kind").is_none(), "{refused}");
    assert!(!cache.exists());
    assert_eq!(refused, run("sync-records", &unknown));
    // The cache cannot be opened.
    private_write(&cache, b"not a database");
    let refused = run("watch", &setup.conversation);
    assert!(refused["cacheRefusal"]["code"].is_string(), "{refused}");
    assert!(refused.get("kind").is_none(), "{refused}");
    assert_eq!(refused, run("sync-records", &setup.conversation));
}

/// Row W17 before registration: `watch`'s discovery answered
/// `source_preparing` is asked again, and the watch registers rather than
/// reporting the row W16 discovery refusal.
#[test]
fn online_watch_registers_after_a_preparing_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, "preparing-head");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let mut child = WatchChild::spawn(vec![
        "watch".into(),
        profile_for(&root, &cache),
        setup.conversation.clone(),
        "100".into(),
        "1".into(),
    ]);
    watch_registered(&mut child);
    watch_pass(&mut child, "recheck", 1);
    assert!(watch_end(child, "passesExhausted"));
}

/// Committed change watches rows L8–L10 and W2–W13, with the example
/// client's own `watch` command on two paired devices in separate processes
/// against one gateway that is never restarted.
#[test]
fn online_two_devices_follow_live_hints_and_converge() {
    // Step 1.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let mut gateway = Gateway::start_live(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let receiver_b = setup.second_receiver.clone().unwrap();
    assert_ne!(receiver_b, setup.receiver);
    let cache_a = setup_cache(&directory.path().join("a"));
    let cache_b = setup_cache(&directory.path().join("b"));
    let profile_a = profile_for(&root, &cache_a);
    let profile_b = profile_for_device(&root, "profile-b.json", &cache_b);
    let watch = |profile: &str, passes: &str| {
        WatchChild::spawn(vec![
            "watch".into(),
            profile.into(),
            setup.conversation.clone(),
            "100".into(),
            passes.into(),
        ])
    };
    let show = |cache: &Path, receiver: &str| {
        let (ok, view) = command(
            vec![
                "show".into(),
                cache.to_string_lossy().into_owned(),
                receiver.into(),
                setup.gateway.clone(),
                setup.conversation.clone(),
            ],
            false,
        );
        assert!(ok, "{view:?}");
        let mut view = view.unwrap();
        // Each `show` names its own rendering with a fresh revision.
        assert!(view["view"]["revision"].take().is_string(), "{view}");
        view
    };

    // Steps 2–3 (W2, W4, W6, L9): A registers; a commit lands while its
    // recheck pass is reading the head (the fixture holds that read until
    // the commit is acknowledged). The recheck already holds it, and its
    // hint, read during that pass, causes exactly one more pass.
    gateway.act("hold");
    let mut a = watch(&profile_a, "2");
    let watch_a = watch_registered(&mut a);
    gateway.act("commit");
    gateway.act("release");
    watch_pass(&mut a, "recheck", 2);
    watch_hint(&mut a, &watch_a, true);
    watch_pass(&mut a, "hint", 2);
    assert!(watch_end(a, "passesExhausted"));

    // Step 4 (L8): two live watchers on one gateway; B's recheck is its
    // initial replay.
    let mut a = watch(&profile_a, "11");
    let mut b = watch(&profile_b, "11");
    let watch_a = watch_registered(&mut a);
    let watch_b = watch_registered(&mut b);
    assert_ne!(watch_a, watch_b);
    watch_pass(&mut a, "recheck", 2);
    watch_pass(&mut b, "recheck", 2);

    // Step 5 (W5, L8, L10): each commit is one hint and one pass per
    // device, timed from the commit's durable acknowledgement.
    let mut samples = vec![];
    for commit in 1..=10 {
        gateway.act("commit");
        let acknowledged = Instant::now();
        for (child, watch) in [(&mut a, &watch_a), (&mut b, &watch_b)] {
            watch_hint(child, watch, false);
            let visible = watch_pass(child, "hint", 2 + commit);
            samples.push(
                visible
                    .saturating_duration_since(acknowledged)
                    .as_secs_f64()
                    * 1000.0,
            );
        }
    }
    // Eleven passes: the recheck and exactly one per commit.
    assert!(watch_end(a, "passesExhausted"));
    assert!(watch_end(b, "passesExhausted"));

    // Step 6 (W4, S2): a commit while nobody is connected is never hinted;
    // the next watch's recheck from the checkpoint recovers it.
    gateway.act("commit");
    let mut a = watch(&profile_a, "2");
    let watch_a = watch_registered(&mut a);
    watch_pass(&mut a, "recheck", 13);
    gateway.act("commit");
    watch_hint(&mut a, &watch_a, false);
    watch_pass(&mut a, "hint", 14);
    assert!(watch_end(a, "passesExhausted"));

    // Step 7 (L5, L8): both devices and a fresh replay hold the same folded
    // view and progress, so neither cache has a duplicate or missing record.
    let mut b = watch(&profile_b, "1");
    watch_registered(&mut b);
    let pass_report = watch_pass_report(&mut b, "recheck", 14).0;
    assert!(watch_end(b, "passesExhausted"));
    let fresh = setup_cache(&directory.path().join("fresh"));
    let (ok, replay) = command(record_command(&root, &fresh, &setup, "100"), false);
    assert!(ok, "{replay:?}");
    // The pass report is the `sync-records` object: the same fields, the
    // device's `enrollment` and `recheck` among them.
    let replay = replay.unwrap();
    let fields = |value: &Value| {
        let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(
        fields(&pass_report),
        fields(&replay),
        "{pass_report} {replay}"
    );
    assert_eq!(
        pass_report["enrollment"]["phase"], "active",
        "{pass_report}"
    );
    assert!(pass_report["recheck"].is_null(), "{pass_report}");
    let view_a = show(&cache_a, &setup.receiver);
    let view_b = show(&cache_b, &receiver_b);
    assert_eq!(view_a["facts"], "14", "{view_a}");
    assert!(view_a["view"].is_object(), "{view_a}");
    assert_eq!(view_a, view_b);
    assert_eq!(view_a, show(&fresh, &setup.receiver));

    // Step 8 (W11, A2): revoking B's receiver closes B's connection with no
    // hint and refuses its next command; A goes on to its pass budget.
    let mut a = watch(&profile_a, "2");
    let mut b = watch(&profile_b, "5");
    let watch_a = watch_registered(&mut a);
    watch_registered(&mut b);
    watch_pass(&mut a, "recheck", 14);
    watch_pass(&mut b, "recheck", 14);
    gateway.act("revoke b");
    gateway.act("commit");
    watch_hint(&mut a, &watch_a, false);
    watch_pass(&mut a, "hint", 15);
    assert!(watch_end(a, "passesExhausted"));
    assert!(!watch_end(b, "connectionClosed"));
    let (ok, denied) = command(
        vec![
            "sync-records".into(),
            profile_b.clone(),
            setup.conversation.clone(),
            "100".into(),
        ],
        false,
    );
    assert!(!ok);
    let denied = denied.unwrap();
    assert_eq!(denied["connectionCheck"], "attempted", "{denied}");
    assert_eq!(denied["enrollmentFailure"]["code"], "refused", "{denied}");
    assert_eq!(show(&cache_b, &receiver_b), view_b);

    // Step 9 (W1, W11, L7): the gateway goes away while A waits for a hint,
    // with nothing run between its recheck and the close. A's watch ends on
    // the close; the next commands fail explicitly before connecting; A's
    // saved view stays readable.
    let view_a = show(&cache_a, &setup.receiver);
    assert_eq!(view_a["facts"], "15", "{view_a}");
    let mut a = watch(&profile_a, "5");
    watch_registered(&mut a);
    watch_pass(&mut a, "recheck", 15);
    drop(gateway);
    assert!(!watch_end(a, "connectionClosed"));
    for args in [
        vec![
            "sync-records".into(),
            profile_a.clone(),
            setup.conversation.clone(),
            "100".into(),
        ],
        vec![
            "watch".into(),
            profile_a.clone(),
            setup.conversation.clone(),
            "100".into(),
            "1".into(),
        ],
    ] {
        let (ok, asleep) = command(args, false);
        assert!(!ok);
        let asleep = asleep.unwrap();
        assert_eq!(asleep["connectionCheck"], "attempted", "{asleep}");
        assert_eq!(asleep["enrollmentFailure"]["code"], "io", "{asleep}");
    }
    assert_eq!(show(&cache_a, &setup.receiver), view_a);

    // Step 10 (L10): the loopback measurement, one machine-readable line.
    samples.sort_by(f64::total_cmp);
    let round = |value: f64| (value * 10.0).round() / 10.0;
    println!(
        "{}",
        json!({"measurement":"commitAckToVisibleMs","transport":"loopback",
            "gateway":"composition fixture",
            "build":if cfg!(debug_assertions) { "debug" } else { "release" },
            "devices":2,"samples":samples.len(),
            "p50":round(percentile(&samples, 50)),"p95":round(percentile(&samples, 95)),
            "max":round(samples[samples.len() - 1]),"passesPerCommitMax":1})
    );
}
