//! Saved output joins captured work with the current cache owner, under competing writes.
use super::fixtures::*;
use crate::read_only_sync::{
    application::{driver::RecordRun, reset::CacheResets, GatewayAttempt, GatewayOutcome},
    domain::CacheReset,
    entrypoint::online::write_records,
};
use nessa_sync::replication::{
    application::ReplicaStore,
    catalogue::{
        CataloguePagePlan, CatalogueStore, EntryKey, ManifestEntry, ManifestPage, ManifestRequest,
        ResolvedEntry,
    },
    domain::{validate_page, Id, Page, PageRequest, Scope},
};
use serde_json::{json, Value};
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn online_saved_projection_handles_competing_owner() {
    let directory = tempfile::tempdir().unwrap();
    let (scope, suffix) = source_records(directory.path()).await;
    let target = suffix.last().unwrap().position;
    let policy = policy();
    let limits = policy.suffix_page();
    for mode in ["stable", "reset", "scope", "delete", "append"] {
        let private = directory.path().join(mode);
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("cache.sqlite3");
        let mut selected = cache(&path);
        let downloaded = if mode == "append" { 1 } else { target };
        selected
            .apply(plan(
                &scope,
                0,
                suffix
                    .iter()
                    .filter(|r| r.position <= downloaded)
                    .cloned()
                    .collect(),
            ))
            .unwrap();
        selected.observe_head(&scope, target).unwrap();
        let mut competitor = cache(&path);
        let before = competitor.cached_progress(&scope).unwrap().unwrap();
        let original = before.clone();
        let change_scope = scope.clone();
        let records = suffix.clone();
        selected.before_projection_refresh(move || match mode {
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
        });
        let attempt = GatewayAttempt {
            result: Some(Ok(RecordRun {
                checked_head: target,
                checked_at_ms: 123_000,
                downloaded,
                pages: 1,
                complete: mode != "append",
            })),
            outcome: GatewayOutcome {
                operation: 1,
                failure: None,
            },
        };
        let saved =
            selected.retained_transcript_state(scope.receiver(), scope.origin(), scope.stream());
        let mut output = vec![];
        let result = write_records(&attempt, saved, None, json!({}), &mut output);
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
        let mut reopened = cache(&path);
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
