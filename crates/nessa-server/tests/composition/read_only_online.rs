//! Real separate client/gateway proofs consume canonical stores and actual paired credentials.
#[path = "read_only_online/fixtures.rs"]
mod fixtures;
use crate::composition::{local_auth::SystemClock, read_only_example};
use crate::product::generated::{
    product_event, product_method, ConversationCloseParams, ConversationRecordsHeadParams,
    ConversationRecordsHeadResult, ConversationRecordsPageParams, ConversationUnwatchParams,
    ConversationWatchRecordsParams, CredentialListParams, RecordPageRequest,
    MAX_CHANGE_WATCH_ID_BYTES, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES, MAX_RECORD_PAGE_PAYLOAD_BYTES,
    MAX_RECORD_PAGE_RECORDS,
};
use crate::product::record_read::wire as record_wire;
use crate::read_only_sync::{
    application::{reset::CacheResets, CachePolicy},
    domain::CacheReset,
    infrastructure::cache::ReadOnlyCache,
};
use fixtures::*;
use nessa_local_database::rusqlite::Connection;
use nessa_sync::replication::{
    application::ReplicaStore,
    catalogue::{
        CataloguePagePlan, CatalogueStore, EntryKey, ManifestEntry, ManifestPage, ManifestRequest,
        ResolvedEntry,
    },
    domain::{validate_page, Id, Limits, Page, PageRequest, Scope},
};
use serde_json::{json, Value};
use std::sync::Arc;
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
    let profile = root.join("profile.json").to_string_lossy().into_owned();
    let cache_root = directory.path().join("cache");
    nessa_local_storage::create_directory(&cache_root).unwrap();
    let cache = cache_root
        .join("cache.sqlite3")
        .to_string_lossy()
        .into_owned();
    let args = |name: &str, target: Option<&str>, pages: Option<&str>| {
        let mut args = vec![name.into(), cache.clone(), profile.clone()];
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
            cache.to_string_lossy().into_owned(),
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
        cache.to_string_lossy().into_owned(),
        root.join("profile.json").to_string_lossy().into_owned(),
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
    let before = std::fs::read(&cache).unwrap();
    let (ok, report) = command(record_command(&root, &cache, &setup, "1"), false);
    assert!(!ok);
    assert_eq!(
        report.unwrap()["transportFailure"]["productCode"],
        "unauthorized"
    );
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

#[test]
fn online_wrong_receiver_or_epoch_does_not_open_cache() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let original: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let private = directory.path().join("cache");
    nessa_local_storage::create_directory(&private).unwrap();
    for (index, field, value) in [
        (0, "receiver", json!(uuid())),
        (1, "accessEpoch", json!(setup.epoch + 1)),
    ] {
        let mut profile = original.clone();
        profile[field] = value;
        let path = root.join(format!("wrong-{index}.json"));
        private_write(&path, &serde_json::to_vec(&profile).unwrap());
        let cache = private.join(format!("absent-{index}.sqlite3"));
        let (ok, report) = command(
            vec![
                "check-records".into(),
                cache.to_string_lossy().into_owned(),
                path.to_string_lossy().into_owned(),
                setup.conversation.clone(),
            ],
            false,
        );
        assert!(!ok);
        assert!(!cache.exists());
        let report = report.unwrap();
        assert_eq!(report["connectionCheck"], "performed");
        assert!(report["transportFailure"]["productCode"].is_string());
    }
}

#[test]
fn online_saved_projection_handles_competing_owner() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let mut wire = WireClient::connect(&root);
    let response = wire.call(
        product_method::CONVERSATION_RECORDS_HEAD,
        &ConversationRecordsHeadParams {
            conversation_id: setup.conversation.clone(),
            receiver_id: setup.receiver.clone(),
            access_epoch: setup.epoch.to_string(),
        },
    );
    let head: ConversationRecordsHeadResult =
        serde_json::from_value(response["payload"].clone()).unwrap();
    let (scope, target) = record_wire::decode_head(head).unwrap();
    let limits = Limits::new(
        1,
        MAX_RECORD_PAGE_PAYLOAD_BYTES,
        MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    )
    .unwrap();
    let policy = CachePolicy::new(32 * 1024 * 1024, 16 * 1024 * 1024, limits).unwrap();
    let mut after = 0;
    let mut suffix = vec![];
    while after < target {
        let request = PageRequest {
            scope: scope.clone(),
            after,
            target,
            max_records: 1,
            max_payload_bytes: limits.max_payload_bytes(),
            max_record_bytes: limits.max_record_bytes(),
        };
        let response = wire.call(
            product_method::CONVERSATION_RECORDS_PAGE,
            &ConversationRecordsPageParams {
                conversation_id: setup.conversation.clone(),
                access_epoch: setup.epoch.to_string(),
                request: record_wire::wire_request(&request),
            },
        );
        let page = record_wire::decode_page_result(
            serde_json::from_value(response["payload"].clone()).unwrap(),
            &request,
        )
        .unwrap();
        after = page.records.last().unwrap().position;
        suffix.extend(page.records);
    }
    for mode in ["stable", "reset", "scope", "delete", "append"] {
        let private = directory.path().join(mode);
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("cache.sqlite3");
        let args = |command: &str, pages: Option<&str>| {
            let mut args = vec![
                command.into(),
                path.to_string_lossy().into_owned(),
                root.join("profile.json").to_string_lossy().into_owned(),
                setup.conversation.clone(),
            ];
            if let Some(pages) = pages {
                args.push(pages.into());
            }
            args
        };
        let mut output = vec![];
        read_only_example::execute(
            &args(
                "sync-records",
                Some(if mode == "append" { "1" } else { "100" }),
            ),
            &mut output,
        )
        .unwrap();
        let mut competitor = ReadOnlyCache::open(&path, policy, Arc::new(SystemClock)).unwrap();
        let before = competitor.cached_progress(&scope).unwrap().unwrap();
        let original = before.clone();
        let change_scope = scope.clone();
        let records = suffix.clone();
        super::BEFORE_SAVED_REFRESH.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || match mode {
                "reset" | "scope" => {
                    let replacement = if mode == "reset" {
                        change_scope.clone()
                    } else {
                        Scope::new(
                            change_scope.receiver().clone(),
                            change_scope.origin().clone(),
                            change_scope.stream().clone(),
                            Id::new("changed").unwrap(),
                            change_scope.schema().clone(),
                            change_scope.access_epoch().clone(),
                        )
                    };
                    let request = CacheReset::new(
                        Id::new("projection-reset").unwrap(),
                        Id::new("operator").unwrap(),
                        change_scope.clone(),
                        before.generation,
                        replacement,
                    )
                    .unwrap();
                    let receipt = competitor.reset_records(&request).unwrap();
                    assert_eq!(receipt.request(), &request);
                    assert_eq!(receipt.after().downloaded, 0);
                }
                "append" => {
                    for record in records
                        .into_iter()
                        .filter(|r| r.position > before.downloaded)
                    {
                        let expected = ReplicaStore::load(&mut competitor, &change_scope)
                            .unwrap()
                            .unwrap();
                        let request = PageRequest {
                            scope: change_scope.clone(),
                            after: expected.position(),
                            target,
                            max_records: 1,
                            max_payload_bytes: limits.max_payload_bytes(),
                            max_record_bytes: limits.max_record_bytes(),
                        };
                        let plan = validate_page(
                            &expected,
                            &request,
                            Page {
                                request: request.clone(),
                                records: vec![record],
                            },
                            limits,
                        )
                        .unwrap();
                        ReplicaStore::apply(&mut competitor, plan).unwrap();
                    }
                }
                "delete" => {
                    let catalogue = Scope::new(
                        change_scope.receiver().clone(),
                        change_scope.origin().clone(),
                        Id::new("catalogue").unwrap(),
                        Id::new("catalogue-incarnation").unwrap(),
                        Id::new("catalogue-schema").unwrap(),
                        change_scope.access_epoch().clone(),
                    );
                    let pass = CatalogueStore::begin(&mut competitor, &catalogue, None, 1)
                        .unwrap()
                        .active
                        .unwrap();
                    let descriptor = ManifestEntry {
                        key: EntryKey {
                            creation: 1,
                            id: change_scope.stream().clone(),
                        },
                        revision: 1,
                        deleted: true,
                    };
                    let plan = CataloguePagePlan::new(
                        ManifestPage {
                            request: ManifestRequest {
                                pass,
                                max_entries: 1,
                            },
                            entries: vec![descriptor.clone()],
                            has_more: false,
                        },
                        vec![ResolvedEntry {
                            manifest: descriptor,
                            payload: vec![],
                        }],
                        vec![],
                    )
                    .unwrap();
                    CatalogueStore::apply_page(&mut competitor, plan).unwrap();
                }
                "stable" => {}
                _ => unreachable!(),
            }))
        });
        let mut output = vec![];
        let result = read_only_example::execute(&args("check-records", None), &mut output);
        let report: Value = serde_json::from_slice(&output).unwrap();
        match mode {
            "stable" => {
                result.unwrap();
                assert_eq!(
                    report["durable"]["progress"]["generation"],
                    original.generation.to_string()
                );
                assert_eq!(
                    report["durable"]["progress"]["downloaded"],
                    target.to_string()
                );
                assert_eq!(report["durable"]["status"], "complete");
            }
            "reset" => {
                result.unwrap();
                assert_eq!(
                    report["durable"]["progress"]["generation"],
                    (original.generation + 1).to_string()
                );
                for field in ["downloaded", "applied", "facts"] {
                    assert_eq!(report["durable"]["progress"][field], "0");
                }
                assert_eq!(report["durable"]["status"], "not_loaded");
            }
            "scope" | "delete" => {
                assert!(result.is_err());
                assert_eq!(
                    report["durable"]["failure"]["code"],
                    if mode == "scope" { "scope" } else { "fenced" }
                );
            }
            "append" => {
                result.unwrap();
                assert_eq!(report["work"]["downloaded"], "1");
                for field in ["downloaded", "applied"] {
                    assert_eq!(report["durable"]["progress"][field], target.to_string());
                }
                assert_eq!(report["durable"]["progress"]["facts"], "1");
                assert_eq!(report["durable"]["status"], "stale");
                assert!(
                    report["durable"]["progress"]["generation"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap()
                        > original.generation
                );
            }
            _ => unreachable!(),
        }
        let mut reopened = ReadOnlyCache::open(&path, policy, Arc::new(SystemClock)).unwrap();
        if mode == "reset" {
            assert_eq!(
                reopened
                    .cached_progress(&scope)
                    .unwrap()
                    .unwrap()
                    .downloaded,
                0
            );
        }
        if mode == "append" {
            assert_eq!(
                reopened.cached_progress(&scope).unwrap().unwrap().applied,
                target
            );
        }
    }
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
            cache_root
                .join("cache.sqlite3")
                .to_string_lossy()
                .into_owned(),
            root.join("profile.json").to_string_lossy().into_owned(),
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
/// full replay produces. Revocation stops hints and the next read is denied;
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
        Frame::Closed(close) => panic!("closed instead of a hint: {close:?}"),
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
    // delivery: the connection closes as authorization lost with no hint,
    // and the next read is denied.
    gateway.act("revoke");
    gateway.act("commit");
    match receiver.frame() {
        Frame::Closed(Some(close)) => assert_eq!(u16::from(close.code), 4004, "{close:?}"),
        Frame::Closed(None) => panic!("closed without a reason"),
        Frame::Text { value, .. } => panic!("frame after revocation: {value}"),
    }
    let (ok, denied) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    assert_eq!(
        denied.unwrap()["transportFailure"]["productCode"],
        "unauthorized"
    );

    // L7: the gateway sleeps. The check fails explicitly, and the saved view
    // is still read offline, unchanged.
    drop(gateway);
    let (ok, asleep) = command(record_command(&root, &cache, &setup, "100"), false);
    assert!(!ok);
    let asleep = asleep.unwrap();
    assert_eq!(
        asleep["configurationFailure"], "endpointUnavailable",
        "{asleep}"
    );
    assert_eq!(asleep["connectionCheck"], "notPerformed", "{asleep}");
    assert_eq!(show(&cache), live_view);
}
