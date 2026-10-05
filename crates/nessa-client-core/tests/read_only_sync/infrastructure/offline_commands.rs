//! Offline commands exercise actual private storage and shared passive projection.
use super::{catalogue_tests, fixtures::*, ReadOnlyCache};
use crate::read_only_sync::domain::CacheReset;
use crate::read_only_sync::entrypoint::{parse, run_local, CommandError};
use nessa_sync::replication::domain::Scope;
use nessa_sync::replication::{application::ReplicaStore, catalogue::CatalogueStore};
use serde_json::Value;
use uuid::Uuid;

fn render(cache: &mut ReadOnlyCache, command: &str, target: &str) -> Value {
    let args = [command, "unused.sqlite3", "receiver", "origin", target]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let command = parse(&args).unwrap();
    let mut output = vec![];
    run_local(&command, cache, Uuid::nil(), &mut output).unwrap();
    assert_eq!(output.last(), Some(&b'\n'));
    serde_json::from_slice(&output).unwrap()
}

#[test]
fn offline_commands_do_not_connect() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "offline.sqlite3");
    let mut cache = cache(&path);
    for (command, target) in [("list", "catalogue"), ("show", scope().stream().as_str())] {
        let value = render(&mut cache, command, target);
        assert_eq!(
            value,
            serde_json::json!({"state":"notLoaded","connectionCheck":"notPerformed"})
        );
        assert!(value.get("view").is_none());
        assert!(value.get("entries").is_none());
    }
    let bad = ["not-a-command", "cache", "receiver", "origin", "catalogue"].map(str::to_owned);
    assert!(matches!(parse(&bad), Err(CommandError::Arguments)));
    let bad = [
        "show",
        "cache",
        "receiver",
        "origin",
        "invalid-conversation",
    ]
    .map(str::to_owned);
    assert!(matches!(parse(&bad), Err(CommandError::Identity)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_show_projects_saved_positions_and_status_together() {
    let root = tempfile::tempdir().unwrap();
    let (scope, records) = source_records(root.path()).await;
    let target = records.last().unwrap().position;
    let path = cache_path(root.path(), "transcript.sqlite3");
    let mut cache = cache(&path);
    cache.apply(plan(&scope, 0, records[..1].to_vec())).unwrap();
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    let partial = render(&mut reopened, "show", scope.stream().as_str());
    assert_eq!(partial["downloaded"], "1");
    assert_eq!(partial["applied"], "0");
    assert_eq!(partial["connectionCheck"], "notPerformed");
    assert_eq!(partial["view"]["transcriptState"], "stale");
    assert!(partial.get("checkedAtMs").is_none());
    for field in ["queue", "steer", "resume", "permissions", "imageInput"] {
        assert_eq!(partial["view"]["capabilities"][field], false);
    }
    reopened
        .apply(plan(&scope, 1, records[1..].to_vec()))
        .unwrap();
    drop(reopened);
    let mut completed = super::fixtures::cache(&path);
    let value = render(&mut completed, "show", scope.stream().as_str());
    assert_eq!(value["downloaded"], target.to_string());
    assert_eq!(value["applied"], target.to_string());
    assert_eq!(value["facts"], "1");
    assert_eq!(value["view"]["transcriptState"], "stale");
    let pass = catalogue_tests::start(&mut completed, 1);
    CatalogueStore::apply_page(
        &mut completed,
        catalogue_tests::page(pass, vec![catalogue_tests::value(1, true)]),
    )
    .unwrap();
    drop(completed);
    let mut deleted = super::fixtures::cache(&path);
    assert_eq!(
        render(&mut deleted, "show", scope.stream().as_str()),
        serde_json::json!({"state":"deleted","connectionCheck":"notPerformed"})
    );
    let list = render(&mut deleted, "list", "catalogue");
    assert_eq!(list["entries"][0]["deleted"], true);
    assert_eq!(list["entries"][0]["metadata"], Value::Null);
}

#[test]
fn offline_list_loaded_empty_is_distinct_from_absent() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "empty.sqlite3");
    let mut cache = cache(&path);
    let scope = catalogue_tests::catalogue_scope();
    CatalogueStore::begin(&mut cache, &scope, None, 0).unwrap();
    drop(cache);
    let mut reopened = super::fixtures::cache(&path);
    let empty = render(&mut reopened, "list", "catalogue");
    assert_eq!(empty["state"], "stale");
    assert_eq!(empty["connectionCheck"], "notPerformed");
    assert_eq!(empty["completedRevision"], "0");
    assert_eq!(empty["entries"], serde_json::json!([]));
    assert_eq!(empty["next"], Value::Null);
}

#[test]
fn offline_catalogue_reset_is_not_empty_source_evidence() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "reset-empty.sqlite3");
    let mut store = cache(&path);
    let old = catalogue_tests::catalogue_scope();
    // Genuine complete zero source observation is the accepted counterpart.
    CatalogueStore::begin(&mut store, &old, None, 0).unwrap();
    let valid = render(&mut store, "list", "catalogue");
    assert_eq!(valid["entries"], serde_json::json!([]));
    let pass = catalogue_tests::start(&mut store, 1);
    CatalogueStore::apply_page(
        &mut store,
        catalogue_tests::page(pass, vec![catalogue_tests::value(1, false)]),
    )
    .unwrap();
    let before = CatalogueStore::progress(&mut store, &old).unwrap().unwrap();
    let next = Scope::new(
        old.receiver().clone(),
        old.origin().clone(),
        old.stream().clone(),
        id("new-incarnation"),
        old.schema().clone(),
        id("new-epoch"),
    );
    let request = CacheReset::new(
        id("reset-live"),
        id("operator"),
        old.clone(),
        before.generation,
        next.clone(),
    )
    .unwrap();
    let receipt = store.reset_catalogue(&request).unwrap();
    assert_eq!(receipt.after().completed, 0);
    drop(store);
    let mut store = cache(&path);
    let repeat = store.reset_catalogue(&request).unwrap();
    assert_eq!(repeat, receipt);
    let reset = render(&mut store, "list", "catalogue");
    assert_eq!(reset["state"], "stale");
    assert_eq!(reset["entries"], serde_json::json!([]));
    // Retained deletion evidence is not erased by reset and is even returned
    // alongside stale inactive progress.
    let expected = CatalogueStore::progress(&mut store, &next).unwrap();
    let pass = CatalogueStore::begin(&mut store, &next, expected, 2)
        .unwrap()
        .active
        .unwrap();
    CatalogueStore::apply_page(
        &mut store,
        catalogue_tests::page(pass, vec![catalogue_tests::value(2, true)]),
    )
    .unwrap();
    let before = CatalogueStore::progress(&mut store, &next)
        .unwrap()
        .unwrap();
    let request = CacheReset::new(
        id("reset-deleted"),
        id("operator"),
        next.clone(),
        before.generation,
        next.clone(),
    )
    .unwrap();
    let receipt = store.reset_catalogue(&request).unwrap();
    assert_eq!(receipt.after().completed, 0);
    drop(store);
    let mut store = cache(&path);
    let deleted = render(&mut store, "list", "catalogue");
    assert_eq!(deleted["state"], "stale");
    assert_eq!(deleted["entries"][0]["deleted"], true);
    assert_eq!(deleted["entries"].as_array().unwrap().len(), 1);
    assert_eq!(valid["state"], "stale");
}
