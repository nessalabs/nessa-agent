//! CLI reset admission and output consume the actual private cache receipt.
use super::{catalogue_tests, fixtures::*, ReadOnlyCache};
use crate::read_only_sync::{
    application::CacheError,
    entrypoint::{parse, run_local, Command, CommandError},
};
use nessa_sync::replication::{
    application::ReplicaStore,
    catalogue::{CataloguePagePlan, CatalogueStore, ManifestPage, ManifestRequest},
    domain::Scope,
};
use serde_json::Value;
use std::io::{self, Write};
use uuid::Uuid;

fn command(name: &str, scope: &Scope, generation: u64) -> Command {
    let args = [
        name,
        "unused",
        scope.receiver().as_str(),
        scope.origin().as_str(),
        scope.stream().as_str(),
        "operation",
        "operator",
        &generation.to_string(),
        scope.incarnation().as_str(),
        scope.schema().as_str(),
        scope.access_epoch().as_str(),
        scope.incarnation().as_str(),
        scope.schema().as_str(),
        "next-epoch",
    ]
    .map(str::to_owned);
    parse(&args).unwrap()
}

fn render(cache: &mut ReadOnlyCache, command: &Command) -> Value {
    let mut output = vec![];
    run_local(command, cache, Uuid::nil(), &mut output).unwrap();
    serde_json::from_slice(&output).unwrap()
}

struct UnavailableOutput;
impl Write for UnavailableOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn reset_command_outputs_original_receipt_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "reset-command.sqlite3");
    let (scope, records) = source_records(root.path()).await;
    let mut store = cache(&path);
    store.apply(plan(&scope, 0, records[..1].to_vec())).unwrap();
    let before = store.cached_progress(&scope).unwrap().unwrap();
    let command = command("reset-records", &scope, before.generation);
    // An output refusal follows a confirmed reset; retry returns that same receipt.
    assert!(matches!(
        run_local(&command, &mut store, Uuid::nil(), &mut UnavailableOutput),
        Err(CommandError::Output)
    ));
    let receipt = render(&mut store, &command);
    assert_eq!(
        receipt["before"]["downloaded"],
        before.downloaded.to_string()
    );
    assert_eq!(receipt["before"]["applied"], before.applied.to_string());
    assert_eq!(receipt["before"]["facts"], before.facts.to_string());
    assert_eq!(
        receipt["before"]["scope"]["epoch"],
        scope.access_epoch().as_str()
    );
    assert_eq!(receipt["after"]["scope"]["epoch"], "next-epoch");
    assert_eq!(
        receipt["after"]["generation"],
        (before.generation + 1).to_string()
    );
    assert_eq!(receipt["after"]["downloaded"], "0");
    assert_eq!(receipt["after"]["applied"], "0");
    assert_eq!(receipt["after"]["facts"], "0");
    assert_eq!(receipt["request"]["operation"], "operation");
    assert_eq!(receipt["request"]["caller"], "operator");
    assert_eq!(receipt["request"]["cause"], "ExplicitReset");
    assert_eq!(receipt["request"]["initiator"], "LocalOperator");
    assert_eq!(receipt["observedAtMs"], 123_000);
    drop(store);
    assert_eq!(render(&mut cache(&path), &command), receipt);
}

#[test]
fn catalogue_reset_command_outputs_original_cursor_and_generation() {
    let root = tempfile::tempdir().unwrap();
    let path = cache_path(root.path(), "catalogue-reset-command.sqlite3");
    let mut store = cache(&path);
    let pass = catalogue_tests::start(&mut store, 3);
    let value = catalogue_tests::value(1, false);
    CatalogueStore::apply_page(
        &mut store,
        CataloguePagePlan::new(
            ManifestPage {
                request: ManifestRequest {
                    pass: pass.clone(),
                    max_entries: 1,
                },
                entries: vec![value.manifest.clone()],
                has_more: true,
            },
            vec![value],
            vec![],
        )
        .unwrap(),
    )
    .unwrap();
    let command = command("reset-catalogue", &pass.scope, pass.generation);
    let receipt = render(&mut store, &command);
    assert_eq!(receipt["before"]["active"]["boundary"], "3");
    assert_eq!(receipt["before"]["completed"], "0");
    assert_eq!(receipt["before"]["active"]["cursor"]["creation"], "1");
    assert_eq!(
        receipt["before"]["active"]["cursor"]["id"],
        scope().stream().as_str()
    );
    assert_eq!(receipt["after"]["completed"], "0");
    assert_eq!(receipt["after"]["active"], Value::Null);
    assert_eq!(
        receipt["after"]["generation"],
        (pass.generation + 1).to_string()
    );
    drop(store);
    assert_eq!(render(&mut cache(&path), &command), receipt);
}

#[test]
fn reset_command_refusal_does_not_output_receipt() {
    let root = tempfile::tempdir().unwrap();
    let mut store = cache(&cache_path(root.path(), "missing-reset.sqlite3"));
    let command = command("reset-records", &scope(), 1);
    let mut output = vec![];
    assert!(matches!(
        run_local(&command, &mut store, Uuid::nil(), &mut output),
        Err(CommandError::Cache(CacheError::Stale))
    ));
    assert!(output.is_empty());
    assert_eq!(store.cached_progress(&scope()).unwrap(), None);
    for generation in ["0", "18446744073709551615", "not-number"] {
        let args = [
            "reset-records",
            "cache",
            "receiver",
            "origin",
            "stream",
            "operation",
            "caller",
            generation,
            "old-inc",
            "schema",
            "old-epoch",
            "new-inc",
            "schema",
            "new-epoch",
        ]
        .map(str::to_owned);
        assert!(matches!(parse(&args), Err(CommandError::Arguments)));
    }
}
