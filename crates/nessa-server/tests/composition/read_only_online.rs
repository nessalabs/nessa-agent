//! Real separate client/gateway proofs consume canonical stores and actual paired credentials.
#[path = "read_only_online/fixtures.rs"]
mod fixtures;
#[path = "read_only_online/native_catalogue.rs"]
mod native_catalogue;
#[path = "read_only_online/native_session.rs"]
mod native_session;
#[path = "read_only_online/relay_reading.rs"]
mod relay_reading;
#[path = "read_only_online/retained_library.rs"]
mod retained_library;
#[path = "read_only_online/semantic.rs"]
mod semantic;
use fixtures::*;
use nessa_client_core::composition;
use nessa_local_database::rusqlite::Connection;
use nessa_protocol::product::generated::{
    product_method, ConversationCloseParams, ConversationRecordsHeadParams,
    ConversationRecordsHeadResult, ConversationRecordsPageParams, CredentialListParams,
    RecordPageRequest, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES,
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
        // Row W1: `follow` is refused at the same point, the same way.
        for args in [
            vec![
                "check-records".into(),
                path.clone(),
                setup.conversation.clone(),
            ],
            vec![
                "follow".into(),
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
/// `fetch` end to end, through the client process: this gateway keeps no
/// held files, so its `artifact.read` refusal comes back typed on one line,
/// the exit is a failure, and nothing is written beside the output.
#[test]
fn online_fetch_refusal_writes_nothing_beside_its_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile = profile_for(&root, &setup_cache(directory.path()));
    let outputs = directory.path().join("outputs");
    std::fs::create_dir(&outputs).unwrap();
    let out = outputs.join("report.txt");
    let (ok, report) = command(
        vec![
            "fetch".into(),
            profile,
            setup.conversation.clone(),
            format!("sha256:{}", "ab".repeat(32)),
            "text/plain".into(),
            "12".into(),
            out.to_string_lossy().into_owned(),
        ],
        false,
    );
    assert!(!ok);
    let report = report.unwrap();
    assert_eq!(report["operation"], "fetch");
    assert_eq!(report["successful"], false);
    assert_eq!(report["connectionCheck"], "performed");
    assert_eq!(
        report["fetchFailure"],
        json!({"code":"gateway","transportFailure":{"code":"heldFile","productCode":"conversations_not_configured"}})
    );
    assert_eq!(report["recheck"], Value::Null);
    assert_eq!(report["enrollment"]["phase"], "active");
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(&outputs).unwrap().count(), 0);
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

/// The next line of a follow, which must be of `kind`, and when it arrived.
fn follow_line(child: &mut FollowChild, kind: &str) -> (Value, Instant) {
    let (line, at) = child.next_line();
    assert_eq!(line["kind"], kind, "{line}");
    (line, at)
}
/// A successful complete pass with this trigger, holding `facts` facts, and
/// when it arrived.
fn follow_pass(child: &mut FollowChild, trigger: &str, facts: u64) -> Instant {
    follow_pass_report(child, trigger, facts).1
}
/// As `follow_pass`, with the pass's report. Attempts the gateway answered
/// `source_preparing` before it (row W17: a slow cold read) are skipped; the
/// pass that follows them carries `trigger: preparing`.
fn follow_pass_report(child: &mut FollowChild, trigger: &str, facts: u64) -> (Value, Instant) {
    let (mut line, mut at) = follow_line(child, "pass");
    let mut trigger = trigger;
    while line["report"]["transportFailure"]["productCode"] == "source_preparing" {
        assert_eq!(line["report"]["successful"], false, "{line}");
        (line, at) = follow_line(child, "pass");
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
/// A view whose cursor moved, on this subscription. `during_pass` is `None`
/// where the gateway's view may come before or after the pass ends.
fn follow_view(child: &mut FollowChild, subscription: &Value, during_pass: Option<bool>) {
    let (line, _) = follow_line(child, "view");
    assert_eq!(line["subscriptionId"], *subscription, "{line}");
    assert!(line["cursor"]["position"].is_string(), "{line}");
    assert!(line["duringPass"].is_boolean(), "{line}");
    if let Some(during_pass) = during_pass {
        assert_eq!(line["duringPass"], during_pass, "{line}");
    }
}
/// The admitted subscription, and the identity it names.
fn follow_subscribed(child: &mut FollowChild) -> Value {
    let (line, _) = follow_line(child, "subscribed");
    assert_eq!(line["enrollment"]["phase"], "active", "{line}");
    assert!(line["subscriptionId"].is_string(), "{line}");
    line["subscriptionId"].clone()
}
fn follow_end(mut child: FollowChild, reason: &str) -> bool {
    let (line, _) = follow_line(&mut child, "ended");
    assert_eq!(line["reason"], reason, "{line}");
    assert!(line["recheck"].is_null(), "{line}");
    child.succeeded()
}
/// Nearest-rank percentile of sorted samples.
fn percentile(sorted: &[f64], percent: usize) -> f64 {
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// Row W16: `follow`'s discovery and cache-open refusals come before
/// `subscribed` and are the objects `sync-records` writes for them: one line,
/// no `kind`, no `ended`; exit 1.
#[test]
fn online_follow_precondition_refusals_use_the_records_output() {
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
        if command_name == "follow" {
            args.push("2".into());
        }
        let (lines, ok) = FollowChild::spawn(args).finish();
        assert!(!ok, "{lines:?}");
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines.into_iter().next().unwrap()
    };
    // Discovery: a conversation this receiver cannot read. Nothing is cached.
    let unknown = uuid();
    let refused = run("follow", &unknown);
    assert_eq!(refused["discoveryFailure"], true, "{refused}");
    assert!(refused.get("kind").is_none(), "{refused}");
    assert!(!cache.exists());
    assert_eq!(refused, run("sync-records", &unknown));
    // The cache cannot be opened.
    private_write(&cache, b"not a database");
    let refused = run("follow", &setup.conversation);
    assert!(refused["cacheRefusal"]["code"].is_string(), "{refused}");
    assert!(refused.get("kind").is_none(), "{refused}");
    assert_eq!(refused, run("sync-records", &setup.conversation));
}

/// Row W17 before subscribing: `follow`'s discovery answered
/// `source_preparing` is asked again, and the follow subscribes rather than
/// reporting the row W16 discovery refusal.
#[test]
fn online_follow_subscribes_after_a_preparing_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, "preparing-head");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let cache = setup_cache(directory.path());
    let mut child = FollowChild::spawn(vec![
        "follow".into(),
        profile_for(&root, &cache),
        setup.conversation.clone(),
        "100".into(),
        "1".into(),
    ]);
    follow_subscribed(&mut child);
    follow_pass(&mut child, "recheck", 1);
    assert!(follow_end(child, "passesExhausted"));
}

/// Read-only sync example rows W2–W13, with the example client's own
/// `follow` command on two paired devices in separate processes
/// against one gateway that is never restarted.
#[test]
fn online_two_devices_follow_live_views_and_converge() {
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
    let follow = |profile: &str, passes: &str| {
        FollowChild::spawn(vec![
            "follow".into(),
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

    // Steps 2–3 (W2, W4, W6): A subscribes; a commit lands while its
    // recheck pass is reading the head (the fixture holds that read until
    // the commit is acknowledged). The recheck already holds it, and the
    // view that moved causes exactly one more pass. That view is taken by
    // the gateway's own subscription task, so it may reach the client during
    // the pass or after it; the session test pins the during-pass case.
    gateway.act("hold");
    let mut a = follow(&profile_a, "2");
    let sub_a = follow_subscribed(&mut a);
    gateway.act("commit");
    gateway.act("release");
    follow_pass(&mut a, "recheck", 2);
    follow_view(&mut a, &sub_a, None);
    follow_pass(&mut a, "view", 2);
    assert!(follow_end(a, "passesExhausted"));

    // Step 4: two live followers on one gateway, each on its own
    // connection (a subscription id names one connection's subscription, so
    // the two may be equal); B's recheck is its initial replay.
    let mut a = follow(&profile_a, "11");
    let mut b = follow(&profile_b, "11");
    let sub_a = follow_subscribed(&mut a);
    let sub_b = follow_subscribed(&mut b);
    follow_pass(&mut a, "recheck", 2);
    follow_pass(&mut b, "recheck", 2);

    // Step 5 (W5): each commit is one moved view and one pass per device,
    // timed from the commit's durable acknowledgement.
    let mut samples = vec![];
    for commit in 1..=10 {
        gateway.act("commit");
        let acknowledged = Instant::now();
        for (child, subscription) in [(&mut a, &sub_a), (&mut b, &sub_b)] {
            follow_view(child, subscription, Some(false));
            let visible = follow_pass(child, "view", 2 + commit);
            samples.push(
                visible
                    .saturating_duration_since(acknowledged)
                    .as_secs_f64()
                    * 1000.0,
            );
        }
    }
    // Eleven passes: the recheck and exactly one per commit.
    assert!(follow_end(a, "passesExhausted"));
    assert!(follow_end(b, "passesExhausted"));

    // Step 6 (W4): a commit while nobody is connected wakes nothing; the
    // next follow's recheck from the checkpoint recovers it.
    gateway.act("commit");
    let mut a = follow(&profile_a, "2");
    let sub_a = follow_subscribed(&mut a);
    follow_pass(&mut a, "recheck", 13);
    gateway.act("commit");
    follow_view(&mut a, &sub_a, Some(false));
    follow_pass(&mut a, "view", 14);
    assert!(follow_end(a, "passesExhausted"));

    // Step 7: both devices and a fresh replay hold the same folded
    // view and progress, so neither cache has a duplicate or missing record.
    let mut b = follow(&profile_b, "1");
    follow_subscribed(&mut b);
    let pass_report = follow_pass_report(&mut b, "recheck", 14).0;
    assert!(follow_end(b, "passesExhausted"));
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

    // Step 8 (W10, A2): revoking B's receiver ends B's subscription
    // `refused` with `unauthorized` before its next view. That authority
    // refusal asks pinned status once. The receiver no longer holds the
    // pairing, so the status is refused and nothing is purged. A goes on to
    // its pass budget.
    let mut a = follow(&profile_a, "2");
    let mut b = follow(&profile_b, "5");
    let sub_a = follow_subscribed(&mut a);
    follow_subscribed(&mut b);
    follow_pass(&mut a, "recheck", 14);
    follow_pass(&mut b, "recheck", 14);
    gateway.act("revoke b");
    gateway.act("commit");
    follow_view(&mut a, &sub_a, Some(false));
    follow_pass(&mut a, "view", 15);
    assert!(follow_end(a, "passesExhausted"));
    let (ended, _) = follow_line(&mut b, "ended");
    assert_eq!(ended["reason"], "subscriptionEnded", "{ended}");
    assert_eq!(ended["ended"]["productCode"], "refused", "{ended}");
    assert_eq!(ended["cause"]["code"], "subscription", "{ended}");
    assert_eq!(ended["cause"]["productCode"], "unauthorized", "{ended}");
    assert_eq!(
        ended["recheck"]["enrollmentFailure"]["code"], "refused",
        "{ended}"
    );
    assert!(ended["recheck"]["purge"].is_null(), "{ended}");
    assert!(!b.succeeded());
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

    // Step 9 (W1, W11): the gateway goes away while A waits for a view,
    // with nothing run between its recheck and the close. A's follow ends on
    // the close; the next commands fail explicitly before connecting; A's
    // saved view stays readable.
    let view_a = show(&cache_a, &setup.receiver);
    assert_eq!(view_a["facts"], "15", "{view_a}");
    let mut a = follow(&profile_a, "5");
    follow_subscribed(&mut a);
    follow_pass(&mut a, "recheck", 15);
    drop(gateway);
    assert!(!follow_end(a, "connectionClosed"));
    for args in [
        vec![
            "sync-records".into(),
            profile_a.clone(),
            setup.conversation.clone(),
            "100".into(),
        ],
        vec![
            "follow".into(),
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

    // Step 10: the loopback measurement, one machine-readable line.
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

/// Rows L11 and PC3/PC6: two paired devices keep catalogues and transcripts.
/// After the gateway process restarts, a commit made before either device
/// reconnects wakes nothing; both `follow` rechecks recover it from
/// the durable checkpoint and agree with each other. Revoking B's credential
/// leaves B's cache intact until B's next command, which reads Terminal and
/// purges only B. A's saved view and next read stay admitted.
#[test]
fn online_two_devices_catch_up_after_restart_and_revocation_purges_only_the_victim() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start_live(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let receiver_b = setup.second_receiver.clone().unwrap();
    assert_ne!(receiver_b, setup.receiver);
    let cache_a = setup_cache(&directory.path().join("a"));
    let cache_b = setup_cache(&directory.path().join("b"));
    let profile_a = profile_for(&root, &cache_a);
    let profile_b = profile_for_device(&root, "profile-b.json", &cache_b);
    let facts = |report: &Value| {
        report["durable"]["progress"]["facts"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    let sync_records = |profile: &str| {
        let (ok, report) = command(
            vec![
                "sync-records".into(),
                profile.into(),
                setup.conversation.clone(),
                "100".into(),
            ],
            false,
        );
        assert!(ok, "{report:?}");
        let report = report.unwrap();
        assert_eq!(report["work"]["complete"], true, "{report}");
        assert_eq!(report["durable"]["status"], "complete", "{report}");
        report
    };
    let sync_catalogue = |profile: &str| {
        let (ok, report) = command(
            vec!["sync-catalogue".into(), profile.into(), "10".into()],
            false,
        );
        assert!(ok, "{report:?}");
        let report = report.unwrap();
        assert_eq!(report["work"]["complete"], true, "{report}");
        report
    };
    let list = |cache: &Path, receiver: &str, report: &Value| {
        let scope = &report["durable"]["progress"]["scope"];
        let (ok, listed) = command(
            vec![
                "list".into(),
                cache.to_string_lossy().into_owned(),
                receiver.into(),
                scope["origin"].as_str().unwrap().into(),
                scope["stream"].as_str().unwrap().into(),
            ],
            false,
        );
        assert!(ok, "{listed:?}");
        let listed = listed.unwrap();
        let mut rows: Vec<(String, bool, String)> = listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["id"].as_str().unwrap().to_owned(),
                    entry["deleted"].as_bool().unwrap(),
                    entry["metadata"]["id"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        rows.sort();
        rows
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
        assert!(view["view"]["revision"].take().is_string(), "{view}");
        view
    };
    let rows = |cache: &Path, receiver: &str, table: &str| -> i64 {
        Connection::open(cache)
            .unwrap()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE receiver = ?1"),
                [receiver],
                |row| row.get(0),
            )
            .unwrap()
    };

    let seeded_a = sync_records(&profile_a);
    let seeded_b = sync_records(&profile_b);
    let seeded_facts = facts(&seeded_a);
    assert_eq!(facts(&seeded_b), seeded_facts);
    let catalogue_a = sync_catalogue(&profile_a);
    let catalogue_b = sync_catalogue(&profile_b);
    let listed_a = list(&cache_a, &setup.receiver, &catalogue_a);
    let listed_b = list(&cache_b, &receiver_b, &catalogue_b);
    assert_eq!(listed_a, listed_b);
    assert!(
        listed_a.iter().any(|(_, _, id)| id == &setup.conversation)
            && listed_a.iter().any(|(_, _, id)| id == &setup.empty),
        "{listed_a:?}"
    );
    let view = show(&cache_a, &setup.receiver);
    assert_eq!(view, show(&cache_b, &receiver_b));

    // The gateway process is gone. Its subscriptions die with it. The restarted
    // process commits before either device connects, so that commit is never
    // seen live; the next recheck is what recovers it.
    drop(gateway);
    let mut gateway = Gateway::start_live(&root);
    gateway.act("commit");
    let catch_up = |profile: &str| {
        let mut child = FollowChild::spawn(vec![
            "follow".into(),
            profile.into(),
            setup.conversation.clone(),
            "100".into(),
            "1".into(),
        ]);
        follow_subscribed(&mut child);
        follow_pass(&mut child, "recheck", seeded_facts + 1);
        assert!(follow_end(child, "passesExhausted"));
    };
    catch_up(&profile_a);
    catch_up(&profile_b);
    let caught_up = show(&cache_a, &setup.receiver);
    assert_eq!(caught_up["facts"], (seeded_facts + 1).to_string());
    assert_eq!(caught_up, show(&cache_b, &receiver_b));
    assert_ne!(caught_up, view);

    // Revocation is not a wipe by itself. B's cache still holds the caught-up
    // view until B's own next command reads the Terminal status.
    gateway.act("revoke-credential b");
    assert_eq!(show(&cache_b, &receiver_b), caught_up);
    assert!(rows(&cache_b, &receiver_b, "transcript_records") > 0);
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
    assert_eq!(denied["enrollment"]["phase"], "terminal", "{denied}");
    assert_eq!(
        denied["enrollment"]["cause"], "credentialRevoked",
        "{denied}"
    );
    assert_eq!(denied["purge"]["receiver"], receiver_b, "{denied}");
    assert_eq!(denied["purge"]["initiator"], "gatewayStatus", "{denied}");
    for table in [
        "transcript_records",
        "transcript_progress",
        "transcript_checkpoints",
        "catalogue_entries",
        "catalogue_progress",
    ] {
        assert_eq!(rows(&cache_b, &receiver_b, table), 0, "{table}");
        assert!(rows(&cache_a, &setup.receiver, table) > 0, "{table}");
    }
    let (ok, again) = command(vec!["sync-catalogue".into(), profile_b, "10".into()], false);
    assert!(!ok);
    assert_eq!(again.unwrap()["configurationFailure"], "notPaired");
    let (ok, still) = command(
        vec![
            "sync-records".into(),
            profile_a,
            setup.conversation.clone(),
            "100".into(),
        ],
        false,
    );
    assert!(ok, "{still:?}");
    assert_eq!(facts(&still.unwrap()), seeded_facts + 1);
    assert_eq!(show(&cache_a, &setup.receiver), caught_up);
}
