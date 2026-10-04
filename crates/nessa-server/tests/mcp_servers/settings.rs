//! Managing the stored MCP servers against the #391 PR 2 state table (rows
//! S2–S10 and S15–S17 here; S1 at the socket, S11–S14 and S18 with the live
//! set): each change audited before its lock and after its effect, written
//! as a whole file under the lock, and the live set replaced only after a
//! publish. And `inspect` (rows I6 and I7, and its audit, here; I1–I4
//! against real server processes beside the inspector, I5 at the socket).
use super::{
    AuditedServer, EditProblem, InspectCut, InspectFailure, InspectedTool, Inspection,
    McpServerAction, McpServerAuditPhase, McpServerCause, McpServerOutcome, McpServerSettingsError,
    ServerNames, ServerProblem, INSPECT_BOUNDS,
};
use crate::mcp_servers::domain::{
    stored_revision, ConfigurationKey, ConfiguredMcpServer, ServerEdit, ServerSave, StdioServer,
};
use crate::mcp_servers::infrastructure::settings_test_support::{
    config, entry, initiator, inspected_over, key, live, managed, server, settings_for,
    settings_over, settings_started_with, LeapingClock, MemoryFiles, RecordingAudit,
    ScriptedInspector, UNPARSEABLE,
};
use crate::mcp_servers::infrastructure::{sdk_server, LaunchSettings, LOCK_WAIT};
use nessa_sdk::infrastructure::{
    acp::sessions::MAX_MCP_SERVERS, clock::RuntimeClock, mcp::MCP_SESSION_VARIABLE,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

fn save(name: &str) -> ServerEdit {
    save_with(name, None, vec![])
}

fn save_with(
    name: &str,
    previous_name: Option<&str>,
    env: Vec<(&str, Option<&str>)>,
) -> ServerEdit {
    ServerEdit::Save(ServerSave {
        previous_name: previous_name.map(str::to_owned),
        server: server(name),
        env: env
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.map(str::to_owned)))
            .collect(),
        enabled: true,
    })
}

fn remove(name: &str) -> ServerEdit {
    ServerEdit::Remove { name: name.into() }
}

/// [`server`]`(name)` as a record names it.
fn audited(name: &str, enabled: bool, env_names: &[&str]) -> Box<AuditedServer> {
    let server = server(name);
    Box::new(AuditedServer {
        name: server.name().to_owned(),
        command: server.command().to_owned(),
        args: server.args().to_vec(),
        enabled,
        env_names: env_names.iter().map(|name| (*name).to_owned()).collect(),
    })
}

/// `server`, on when `enabled`, given `env`.
fn configured(server: StdioServer, enabled: bool, env: &[(&str, &str)]) -> ConfiguredMcpServer {
    let env = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()));
    ConfiguredMcpServer::new(server, enabled, env).unwrap()
}

/// The stored servers' names in `files`.
fn stored(files: &MemoryFiles) -> Vec<String> {
    files.document()["agents"]["mcpServers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect()
}

/// The outcome recorded last, after a requested record for the same change.
fn outcome(audit: &RecordingAudit) -> McpServerOutcome {
    let records = audit.records();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].phase, McpServerAuditPhase::Requested);
    assert_eq!(records[0].operation_id, records[1].operation_id);
    match &records[1].phase {
        McpServerAuditPhase::Outcome(outcome) => outcome.clone(),
        McpServerAuditPhase::Requested => panic!("no outcome"),
    }
}

#[tokio::test]
async fn a_save_is_published_then_replaces_the_live_set_and_is_audited_both_sides() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let before = settings.list().await.unwrap();
    assert_eq!(live(&servers), ["nessa"]);
    let after = settings
        .edit(
            initiator(),
            before.revision.clone(),
            save_with("b", None, vec![("API_TOKEN", Some("secret-value"))]),
        )
        .await
        .unwrap();
    assert_ne!(after, before.revision);
    assert_eq!(settings.list().await.unwrap().revision, after);
    assert_eq!(stored(&files), ["a", "b"]);
    assert_eq!(live(&servers), ["a", "b", "nessa"]);
    // The rest of the file is as it was, and the value is stored.
    let document = files.document();
    assert_eq!(document["session"], json!({"writeTimeoutMs": 75}));
    assert_eq!(document["agents"]["catalog"], "/models.json");
    assert_eq!(
        document["agents"]["mcpServers"][1]["env"],
        json!({"API_TOKEN": "secret-value"})
    );
    let records = audit.records();
    assert_eq!(
        records[0].cause,
        McpServerCause::CallerRequested(initiator())
    );
    assert_eq!(records[1].cause, records[0].cause);
    assert_eq!(records[0].request.action, McpServerAction::Save);
    assert_eq!(records[0].request.target, "b");
    assert_eq!(records[0].request.revision, before.revision);
    assert_eq!(
        records[0].request.server,
        Some(audited("b", true, &["API_TOKEN"]))
    );
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Applied {
            before: ServerNames {
                revision: before.revision,
                names: vec!["a".into()],
                target: None,
            },
            after: ServerNames {
                revision: after,
                names: vec!["a".into(), "b".into()],
                target: Some(audited("b", true, &["API_TOKEN"])),
            },
            live_set_replaced: true,
            durable: true,
        }
    );
    // Names only, never a value, in any record.
    assert!(!format!("{records:?}").contains("secret-value"));
}

/// The list: the stored servers in order, then the managed one; variable
/// names and never their values.
#[tokio::test]
async fn the_list_names_each_variable_and_marks_the_managed_server() {
    let mut stored = entry("a");
    stored["env"] = json!({"B": "secret-b", "A": "secret-a"});
    stored["enabled"] = json!(false);
    let files = MemoryFiles::holding(config(vec![stored]));
    let (settings, _) = settings_for(files, Arc::new(RecordingAudit::default()));
    let list = settings.list().await.unwrap();
    let rows: Vec<_> = list
        .servers
        .iter()
        .map(|row| (row.server.name(), row.enabled, row.managed))
        .collect();
    assert_eq!(rows, [("a", false, false), ("nessa", true, true)]);
    assert_eq!(list.servers[0].env_names, ["A", "B"]);
    assert!(!format!("{list:?}").contains("secret"));
}

/// S2: a stale revision is refused with the current one, after the
/// requested record and before anything is written.
#[tokio::test]
async fn s2_a_stale_revision_is_refused_with_the_current_one_and_nothing_is_written() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let current = settings.list().await.unwrap().revision;
    assert_eq!(
        settings
            .edit(initiator(), "sha256:stale".into(), save("b"))
            .await,
        Err(McpServerSettingsError::RevisionConflict {
            revision: current.clone()
        })
    );
    assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
    assert_eq!(live(&servers), ["nessa"]);
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Refused {
            reason: "revision_conflict",
            before: Some(ServerNames {
                revision: current,
                names: vec!["a".into()],
                target: None,
            }),
        }
    );
}

/// S3: two saves at one revision at once are serialised by the lock; the
/// first wins and the second is refused with the first's revision. Writer A
/// is held inside the window between its re-read and its publish: B, waiting
/// on the lock, reads nothing until A has published — without the lock, B
/// would read the old revision there, and both would publish.
#[tokio::test]
async fn s3_two_saves_at_one_revision_are_serialised_and_the_second_conflicts() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_over(files.clone(), audit, Arc::new(RuntimeClock::new()));
    let settings = Arc::new(settings);
    let revision = settings.list().await.unwrap().revision;
    let (release, gate) = std::sync::mpsc::channel();
    *files.publish_gate.lock().unwrap() = Some(gate);
    let first = tokio::spawn({
        let settings = settings.clone();
        let revision = revision.clone();
        async move { settings.edit(initiator(), revision, save("a")).await }
    });
    within("A reaches its publish", || {
        files.publishing.load(Ordering::SeqCst)
    })
    .await;
    let reads = files.reads.load(Ordering::SeqCst);
    let second = tokio::spawn({
        let settings = settings.clone();
        let revision = revision.clone();
        async move { settings.edit(initiator(), revision, save("b")).await }
    });
    // Real time, well inside the lock's two-second wait, for a B that does
    // not wait to show that it read.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        files.reads.load(Ordering::SeqCst),
        reads,
        "B read while A held the lock between its re-read and its publish"
    );
    release.send(()).unwrap();
    let won = bounded(first).await.unwrap();
    let lost = bounded(second).await.unwrap_err();
    assert_eq!(
        lost,
        McpServerSettingsError::RevisionConflict {
            revision: won.clone()
        }
    );
    assert_eq!(files.publishes.load(Ordering::SeqCst), 1);
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["a", "nessa"]);
}

/// Wait, five real seconds at most, until `done`.
async fn within(what: &str, done: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(5), "never: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// `task`'s answer, within ten real seconds: a test fails rather than hangs.
async fn bounded<T>(task: tokio::task::JoinHandle<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("answered in time")
        .unwrap()
}

/// S4: a lock held past its bound is `busy`, and nothing is written.
#[tokio::test]
async fn s4_a_lock_held_past_its_bound_is_busy_and_nothing_is_written() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    files.held.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), revision, save("a")).await,
        Err(McpServerSettingsError::Busy)
    );
    assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
    assert_eq!(live(&servers), ["nessa"]);
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Failed {
            reason: "busy",
            before: None
        }
    );
}

/// S5: a publish that fails leaves the old file and the live set, and is
/// audited as failed.
#[tokio::test]
async fn s5_a_failed_publish_keeps_the_old_file_and_live_set() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    let old = files.current();
    files.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), revision, save("b")).await,
        Err(McpServerSettingsError::StorageUnavailable { applied: false })
    );
    assert_eq!(files.current(), old);
    assert_eq!(live(&servers), ["nessa"]);
    assert!(matches!(
        outcome(&audit),
        McpServerOutcome::Failed {
            reason: "storage_unavailable",
            before: Some(_)
        }
    ));
}

/// S6: when the requested record cannot be written, nothing is locked,
/// written or applied.
#[tokio::test]
async fn s6_an_unwritable_requested_record_stops_everything() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    audit.fail_requested.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), revision, save("a")).await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: false,
            cause: None,
        })
    );
    assert_eq!(files.locks.load(Ordering::SeqCst), 0);
    assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
    assert_eq!(live(&servers), ["nessa"]);
    assert!(audit.records().is_empty());
}

/// S7: published, then the outcome record fails: `audit_unavailable` with
/// `applied`, and the file and live set are the new ones. A refusal or
/// failure whose outcome cannot be recorded says it did not apply and
/// carries its own cause, so neither is lost (pass 2b, decision 4).
#[tokio::test]
async fn s7_an_unwritable_outcome_after_a_publish_says_it_applied() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    audit.fail_outcome.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), revision, save("a")).await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: true,
            cause: None,
        })
    );
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["a", "nessa"]);
    let revision = settings.list().await.unwrap().revision;
    assert_eq!(
        settings
            .edit(initiator(), revision.clone(), remove("unknown"))
            .await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: false,
            cause: Some(Box::new(McpServerSettingsError::NotFound)),
        })
    );
    assert_eq!(
        settings
            .edit(initiator(), "stale".into(), remove("a"))
            .await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: false,
            cause: Some(Box::new(McpServerSettingsError::RevisionConflict {
                revision: revision.clone()
            })),
        })
    );
    files.fail_publish.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), revision, remove("a")).await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: false,
            cause: Some(Box::new(McpServerSettingsError::StorageUnavailable {
                applied: false
            })),
        })
    );
}

/// S-sync: the file is replaced, then its directory cannot be synced. The
/// change is applied — the file is new — so the live set follows it, and
/// the outcome is `applied` with `durable: false`; the caller is answered
/// `storage_unavailable` with `applied: true`. When that outcome cannot be
/// recorded either, `audit_unavailable` keeps the storage failure as its
/// cause.
#[tokio::test]
async fn s_sync_a_publish_whose_directory_sync_fails_is_applied_not_durable() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let before = settings.list().await.unwrap().revision;
    files.fail_sync.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), before.clone(), save("a")).await,
        Err(McpServerSettingsError::StorageUnavailable { applied: true })
    );
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["a", "nessa"]);
    let after = settings.list().await.unwrap().revision;
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Applied {
            before: ServerNames {
                revision: before,
                names: vec![],
                target: None,
            },
            after: ServerNames {
                revision: after.clone(),
                names: vec!["a".into()],
                target: Some(audited("a", true, &[])),
            },
            live_set_replaced: true,
            durable: false,
        }
    );
    audit.fail_outcome.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), after, remove("a")).await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: true,
            cause: Some(Box::new(McpServerSettingsError::StorageUnavailable {
                applied: true
            })),
        })
    );
    assert!(stored(&files).is_empty());
    assert_eq!(live(&servers), ["nessa"]);
}

/// C-null: `"agents": null` is no block. The writer reads it as the
/// runtime configuration does — absent — so the list is empty and a save
/// writes a block from the running catalog and workspace, rather than
/// refusing the file.
#[tokio::test]
async fn c_null_agents_is_no_block_to_the_store() {
    let files = MemoryFiles::holding(json!({"session": {"writeTimeoutMs": 75}, "agents": null}));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit);
    let list = settings.list().await.unwrap();
    assert_eq!(list.servers.len(), 1, "only the managed server");
    settings
        .edit(initiator(), list.revision, save("a"))
        .await
        .unwrap();
    let document = files.document();
    assert_eq!(document["agents"]["catalog"], "/models.json");
    assert_eq!(document["agents"]["workspace"], "/w");
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["a", "nessa"]);
}

/// A live set whose `replace` waits, holding the change that called it
/// there, until it is let go.
struct HeldLive {
    live: crate::mcp_servers::infrastructure::LiveMcpServers,
    replacing: std::sync::atomic::AtomicBool,
    gate: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}
impl super::LiveServerSet for HeldLive {
    fn managed(&self) -> Option<ConfiguredMcpServer> {
        self.live.managed()
    }
    fn problem(&self, stored: &[ConfiguredMcpServer]) -> Option<ServerProblem> {
        self.live.problem(stored)
    }
    fn replace(&self, stored: &[ConfiguredMcpServer]) -> Result<(), super::LiveSetKept> {
        self.replacing.store(true, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
        self.live.replace(stored)
    }
}

/// The lock is let go only after the live set is replaced: a second writer
/// cannot take it — it does not even read — while the first writer's
/// replacement is under way, so two changes replace in the order they
/// published. Let go before the replacement, the second would read the new
/// file, publish and replace, and the first's older set could land last.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_writer_waits_for_the_first_writers_live_replace() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (release, gate) = std::sync::mpsc::channel();
    let held = Arc::new(std::sync::Mutex::new(None::<Arc<HeldLive>>));
    let (settings, servers) = super::super::infrastructure::settings_test_support::settings_through(
        files.clone(),
        audit,
        Arc::new(RuntimeClock::new()),
        Arc::new(ScriptedInspector::default()),
        {
            let held = held.clone();
            move |live| {
                let live = Arc::new(HeldLive {
                    live,
                    replacing: Default::default(),
                    gate: std::sync::Mutex::new(Some(gate)),
                });
                *held.lock().unwrap() = Some(live.clone());
                live
            }
        },
    );
    let live_set = held.lock().unwrap().clone().unwrap();
    let settings = Arc::new(settings);
    let revision = settings.list().await.unwrap().revision;
    let first = tokio::spawn({
        let settings = settings.clone();
        let revision = revision.clone();
        async move { settings.edit(initiator(), revision, save("a")).await }
    });
    within("the first writer replaces the live set", || {
        live_set.replacing.load(Ordering::SeqCst)
    })
    .await;
    let reads = files.reads.load(Ordering::SeqCst);
    let second = tokio::spawn({
        let settings = settings.clone();
        async move { settings.edit(initiator(), revision, save("b")).await }
    });
    // Real time, well inside the lock's two-second wait.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        files.locks.load(Ordering::SeqCst),
        1,
        "the second took the lock"
    );
    assert_eq!(files.reads.load(Ordering::SeqCst), reads, "the second read");
    assert!(!second.is_finished());
    release.send(()).unwrap();
    let won = bounded(first).await.unwrap();
    assert_eq!(
        bounded(second).await,
        Err(McpServerSettingsError::RevisionConflict { revision: won })
    );
    assert_eq!(live(&servers), ["a", "nessa"]);
}

/// S14 for a change (pass 2b, decision 5): a publish that lands while the
/// gateway stops answers success — the file is the gateway's, and the next
/// start reads it — with the live set not replaced, as its outcome record
/// says.
#[tokio::test]
async fn a_publish_during_stop_answers_success_and_leaves_the_live_set() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let before = settings.list().await.unwrap().revision;
    servers.stop().await;
    let after = settings
        .edit(initiator(), before.clone(), save("a"))
        .await
        .unwrap();
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["nessa"]);
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Applied {
            before: ServerNames {
                revision: before,
                names: vec![],
                target: None,
            },
            after: ServerNames {
                revision: after,
                names: vec!["a".into()],
                target: Some(audited("a", true, &[])),
            },
            live_set_replaced: false,
            durable: true,
        }
    );
}

/// S8: a configuration that does not parse — before the edit, as its block,
/// or after it — is refused and never repaired.
#[tokio::test]
async fn s8_a_configuration_that_does_not_parse_is_refused_and_never_repaired() {
    let mut unknown = entry("a");
    unknown["url"] = json!("https://example.com");
    for files in [
        MemoryFiles::raw(b"not json"),
        MemoryFiles::raw(b"[]"),
        MemoryFiles::holding(config(vec![unknown])),
        MemoryFiles::holding(json!({"agents": {"mcpServers": "a"}})),
    ] {
        let audit = Arc::new(RecordingAudit::default());
        let (settings, servers) = settings_for(files.clone(), audit.clone());
        let old = files.current();
        assert_eq!(
            settings.list().await,
            Err(McpServerSettingsError::ConfigInvalid)
        );
        assert_eq!(
            settings.edit(initiator(), "any".into(), save("b")).await,
            Err(McpServerSettingsError::ConfigInvalid)
        );
        assert_eq!(files.current(), old);
        assert_eq!(live(&servers), ["nessa"]);
        assert!(matches!(
            outcome(&audit),
            McpServerOutcome::Refused {
                reason: "config_invalid",
                before: None
            }
        ));
    }
    // After: the edited file is one the runtime configuration refuses.
    let files = MemoryFiles::holding(config(vec![]));
    let (settings, servers) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
    let revision = settings.list().await.unwrap().revision;
    let refused = StdioServer::new("b", server("b").command(), vec![UNPARSEABLE.into()]);
    let edit = ServerEdit::Save(ServerSave {
        previous_name: None,
        server: refused,
        env: vec![],
        enabled: true,
    });
    let old = files.current();
    assert_eq!(
        settings.edit(initiator(), revision, edit).await,
        Err(McpServerSettingsError::ConfigInvalid)
    );
    assert_eq!(files.current(), old);
    assert_eq!(live(&servers), ["nessa"]);
}

/// S9: a result past the bound is refused, and so is a file already past
/// it; nothing is written.
#[tokio::test]
async fn s9_a_result_past_the_bound_is_refused_and_nothing_is_written() {
    let files = MemoryFiles::holding(config(vec![]));
    let (settings, servers) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
    let revision = settings.list().await.unwrap().revision;
    let large = StdioServer::new("b", server("b").command(), vec!["x".repeat(4096)]);
    let edit = ServerEdit::Save(ServerSave {
        previous_name: None,
        server: large,
        env: vec![],
        enabled: true,
    });
    let old = files.current();
    assert_eq!(
        settings.edit(initiator(), revision, edit).await,
        Err(McpServerSettingsError::ConfigTooLarge)
    );
    assert_eq!(files.current(), old);
    assert_eq!(live(&servers), ["nessa"]);
    let mut padded = config(vec![]);
    padded["pad"] = json!("x".repeat(5000));
    let files = MemoryFiles::holding(padded);
    let (settings, _) = settings_for(files, Arc::new(RecordingAudit::default()));
    assert_eq!(
        settings.list().await,
        Err(McpServerSettingsError::ConfigTooLarge)
    );
}

/// S10: a 17th server, a bad name, a duplicate, `nessa`, and a bad or
/// reserved variable name are refused with the typed problem (or as
/// reserved), and nothing is written.
#[tokio::test]
async fn s10_an_invalid_or_reserved_server_is_refused_with_its_problem() {
    // The managed server counts: 15 stored and it make 16.
    let full: Vec<Value> = (0..MAX_MCP_SERVERS - 1)
        .map(|index| entry(&format!("s{index}")))
        .collect();
    let cases: Vec<(Vec<Value>, ServerEdit, McpServerSettingsError)> = vec![
        (
            full,
            save("one-more"),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::TooMany)),
        ),
        (
            vec![],
            save("bad name"),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::Name {
                server: "bad name".into(),
            })),
        ),
        (
            vec![entry("a"), entry("b")],
            save_with("b", Some("a"), vec![]),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::DuplicateName {
                server: "b".into(),
            })),
        ),
        (vec![], save("nessa"), McpServerSettingsError::ReservedName),
        (
            vec![entry("a")],
            save_with("a", Some("nessa"), vec![]),
            McpServerSettingsError::ReservedName,
        ),
        (
            vec![],
            remove("nessa"),
            McpServerSettingsError::ReservedName,
        ),
        (
            vec![],
            save_with("a", None, vec![("1BAD", Some("v"))]),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::EnvironmentName {
                server: "a".into(),
                name: "1BAD".into(),
            })),
        ),
        (
            vec![],
            save_with("a", None, vec![(MCP_SESSION_VARIABLE, Some("token"))]),
            McpServerSettingsError::Invalid(EditProblem::Server(
                ServerProblem::ReservedEnvironmentName {
                    server: "a".into(),
                    name: MCP_SESSION_VARIABLE.into(),
                },
            )),
        ),
        (
            vec![],
            save_with("a", None, vec![("KEY", Some("a\0b"))]),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::EnvironmentValue {
                server: "a".into(),
                name: "KEY".into(),
            })),
        ),
        (
            vec![],
            save_with("a", None, vec![("KEY", Some("1")), ("KEY", Some("2"))]),
            McpServerSettingsError::Invalid(EditProblem::EnvironmentNameRepeated {
                server: "a".into(),
                name: "KEY".into(),
            }),
        ),
    ];
    for (stored, edit, refusal) in cases {
        let files = MemoryFiles::holding(config(stored));
        let (settings, servers) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
        let revision = settings.list().await.unwrap().revision;
        let before = live(&servers);
        let old = files.current();
        assert_eq!(
            settings.edit(initiator(), revision, edit.clone()).await,
            Err(refusal),
            "{edit:?}"
        );
        assert_eq!(files.current(), old, "{edit:?}");
        assert_eq!(live(&servers), before);
    }
}

/// S15: a rename is one write, the old name gone and the new one in its
/// place with its kept variables; an unknown previous name is `not_found`.
#[tokio::test]
async fn s15_a_rename_is_one_write_and_an_unknown_previous_name_is_not_found() {
    let mut old_entry = entry("old");
    old_entry["env"] = json!({"TOKEN": "kept"});
    let files = MemoryFiles::holding(config(vec![old_entry, entry("other")]));
    let (settings, servers) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
    let revision = settings.list().await.unwrap().revision;
    let revision = settings
        .edit(
            initiator(),
            revision,
            save_with("new", Some("old"), vec![("TOKEN", None)]),
        )
        .await
        .unwrap();
    assert_eq!(files.publishes.load(Ordering::SeqCst), 1);
    assert_eq!(stored(&files), ["new", "other"]);
    assert_eq!(
        files.document()["agents"]["mcpServers"][0]["env"],
        json!({"TOKEN": "kept"})
    );
    assert_eq!(live(&servers), ["nessa", "new", "other"]);
    assert_eq!(
        settings
            .edit(
                initiator(),
                revision,
                save_with("x", Some("missing"), vec![])
            )
            .await,
        Err(McpServerSettingsError::NotFound)
    );
    assert_eq!(files.publishes.load(Ordering::SeqCst), 1);
}

/// S16: removing an unknown name is `not_found`, and nothing is written.
#[tokio::test]
async fn s16_removing_an_unknown_name_is_not_found() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    assert_eq!(
        settings.edit(initiator(), revision, remove("b")).await,
        Err(McpServerSettingsError::NotFound)
    );
    assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
    assert_eq!(live(&servers), ["nessa"]);
    assert!(matches!(
        outcome(&audit),
        McpServerOutcome::Refused {
            reason: "not_found",
            ..
        }
    ));
}

/// S17: a variable given without a value keeps the stored one; one with no
/// stored value is invalid. A variable left out is removed.
#[tokio::test]
async fn s17_a_null_value_keeps_the_stored_one_and_needs_one_to_keep() {
    let mut stored_entry = entry("a");
    stored_entry["env"] = json!({"KEEP": "stored", "DROP": "gone"});
    let files = MemoryFiles::holding(config(vec![stored_entry]));
    let (settings, _) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
    let revision = settings.list().await.unwrap().revision;
    let revision = settings
        .edit(
            initiator(),
            revision,
            save_with("a", None, vec![("KEEP", None), ("NEW", Some("fresh"))]),
        )
        .await
        .unwrap();
    assert_eq!(
        files.document()["agents"]["mcpServers"][0]["env"],
        json!({"KEEP": "stored", "NEW": "fresh"})
    );
    let old = files.current();
    assert_eq!(
        settings
            .edit(
                initiator(),
                revision,
                save_with("a", None, vec![("NEVER", None)])
            )
            .await,
        Err(McpServerSettingsError::Invalid(
            EditProblem::EnvironmentValueMissing {
                server: "a".into(),
                name: "NEVER".into()
            }
        ))
    );
    assert_eq!(files.current(), old);
}

/// A server turned off stays stored and leaves the live set; turned on, it
/// is back.
#[tokio::test]
async fn a_disabled_server_stays_stored_and_out_of_the_live_set() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let (settings, servers) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
    let revision = settings.list().await.unwrap().revision;
    let off = ServerEdit::Save(ServerSave {
        previous_name: None,
        server: server("a"),
        env: vec![],
        enabled: false,
    });
    let revision = settings.edit(initiator(), revision, off).await.unwrap();
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(
        files.document()["agents"]["mcpServers"][0]["enabled"],
        false
    );
    assert_eq!(live(&servers), ["nessa"]);
    settings
        .edit(initiator(), revision, save("a"))
        .await
        .unwrap();
    assert_eq!(live(&servers), ["a", "nessa"]);
}

/// The managed server is the desktop's, in memory: a write never adds it to
/// the file. A file with no `agents` block gains one from the gateway's own
/// catalog and workspace; one with no file at all is written fresh.
#[tokio::test]
async fn a_write_never_stores_the_managed_server_and_starts_a_missing_block() {
    for files in [
        MemoryFiles::holding(json!({"session": {"writeTimeoutMs": 75}})),
        Arc::new(MemoryFiles::default()),
    ] {
        let (settings, servers) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
        let list = settings.list().await.unwrap();
        assert_eq!(list.servers.len(), 1);
        assert!(list.servers[0].managed);
        settings
            .edit(initiator(), list.revision, save("a"))
            .await
            .unwrap();
        let document = files.document();
        assert_eq!(document["agents"]["catalog"], "/models.json");
        assert_eq!(document["agents"]["workspace"], "/w");
        assert_eq!(stored(&files), ["a"]);
        assert_eq!(live(&servers), ["a", "nessa"]);
    }
}

/// The revision is a digest of the stored block alone: the rest of the file
/// can change under it, and an absent block is the empty one.
#[tokio::test]
async fn the_revision_is_the_stored_blocks_digest() {
    let revision = |document: Value| async move {
        let files = MemoryFiles::holding(document);
        let (settings, _) = settings_for(files, Arc::new(RecordingAudit::default()));
        settings.list().await.unwrap().revision
    };
    let base = revision(config(vec![entry("a")])).await;
    let mut other = config(vec![entry("a")]);
    other["session"] = json!({"writeTimeoutMs": 99});
    assert_eq!(revision(other).await, base);
    assert_ne!(revision(config(vec![entry("b")])).await, base);
    assert_eq!(revision(json!({})).await, revision(config(vec![])).await);
}

/// The revision is keyed with the process's key: it changes when only a
/// variable's value does, yet is not the unkeyed digest of the block — so it
/// gives nothing to test a guessed value against — and another key (another
/// run of the gateway) gives another revision for the same block.
#[tokio::test]
async fn the_revision_is_keyed_and_changes_with_a_variables_value() {
    let with_value = |value: &str| {
        let mut stored = entry("a");
        stored["env"] = json!({"TOKEN": value});
        MemoryFiles::holding(config(vec![stored]))
    };
    let files = with_value("one");
    let (settings, _) = settings_for(files.clone(), Arc::new(RecordingAudit::default()));
    let one = settings.list().await.unwrap().revision;
    let (settings, _) = settings_for(with_value("two"), Arc::new(RecordingAudit::default()));
    assert_ne!(settings.list().await.unwrap().revision, one);
    let block = serde_json::to_vec(&files.document()["agents"]["mcpServers"]).unwrap();
    let unkeyed = format!("{:x}", Sha256::digest(&block));
    assert!(!one.contains(&unkeyed), "{one}");
    assert_eq!(one, stored_revision(&key(), &block));
    assert_ne!(
        stored_revision(&ConfigurationKey::new([8; 32]), &block),
        one
    );
}

/// The record of each applied change names its target before and after —
/// command, arguments, whether it is on, and its variables' names, never a
/// value: a rename (before under the old name), turning it off, adding one,
/// and removing one.
#[tokio::test]
async fn the_audit_records_the_targets_before_and_after_on_save_rename_disable_and_remove() {
    let mut stored = entry("a");
    stored["env"] = json!({"TOKEN": "secret-a"});
    let files = MemoryFiles::holding(config(vec![stored]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, _) = settings_for(files, audit.clone());
    let targets = |audit: &RecordingAudit| match audit.records().last().map(|r| r.phase.clone()) {
        Some(McpServerAuditPhase::Outcome(McpServerOutcome::Applied { before, after, .. })) => {
            (before.target, after.target)
        }
        other => panic!("not applied: {other:?}"),
    };
    let revision = settings.list().await.unwrap().revision;
    let revision = settings
        .edit(
            initiator(),
            revision,
            save_with(
                "b",
                Some("a"),
                vec![("TOKEN", None), ("NEW", Some("secret-new"))],
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        targets(&audit),
        (
            Some(audited("a", true, &["TOKEN"])),
            Some(audited("b", true, &["NEW", "TOKEN"]))
        )
    );
    // The request names the variables sorted by name, as they are stored,
    // whatever order they were given in.
    assert_eq!(
        audit.records()[0].request.server,
        Some(audited("b", true, &["NEW", "TOKEN"]))
    );
    let off = ServerEdit::Save(ServerSave {
        previous_name: None,
        server: server("b"),
        env: vec![("TOKEN".into(), None), ("NEW".into(), None)],
        enabled: false,
    });
    let revision = settings.edit(initiator(), revision, off).await.unwrap();
    assert_eq!(
        targets(&audit),
        (
            Some(audited("b", true, &["NEW", "TOKEN"])),
            Some(audited("b", false, &["NEW", "TOKEN"]))
        )
    );
    let revision = settings
        .edit(initiator(), revision, save("c"))
        .await
        .unwrap();
    assert_eq!(targets(&audit), (None, Some(audited("c", true, &[]))));
    settings
        .edit(initiator(), revision, remove("b"))
        .await
        .unwrap();
    assert_eq!(
        targets(&audit),
        (Some(audited("b", false, &["NEW", "TOKEN"])), None)
    );
    assert!(!format!("{:?}", audit.records()).contains("secret"));
}

/// An edit made to the file outside the lock, after the change read it and
/// before it writes, is not overwritten: the write re-reads, finds another
/// revision, and refuses `revision_conflict` with it; nothing is written and
/// the live set is kept.
#[tokio::test]
async fn a_change_made_outside_the_lock_after_the_read_is_a_conflict() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    let elsewhere = serde_json::to_vec(&config(vec![entry("a"), entry("x")])).unwrap();
    *files.after_next_read.lock().unwrap() = Some(elsewhere.clone());
    let refused = settings.edit(initiator(), revision, save("b")).await;
    let current = settings.list().await.unwrap().revision;
    assert_eq!(
        refused,
        Err(McpServerSettingsError::RevisionConflict { revision: current })
    );
    assert_eq!(files.current(), Some(elsewhere));
    assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
    assert_eq!(live(&servers), ["nessa"]);
    assert!(matches!(
        outcome(&audit),
        McpServerOutcome::Refused {
            reason: "revision_conflict",
            ..
        }
    ));
}

/// A change whose caller goes away while its write is under way is not
/// abandoned: it is owned by its own task, which keeps the lock until the
/// write has finished, then replaces the live set and records its outcome —
/// so the file and the live set agree, and the record does not depend on the
/// caller.
#[tokio::test]
async fn a_caller_gone_mid_write_still_replaces_the_live_set_and_records_the_outcome() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let settings = Arc::new(settings);
    let revision = settings.list().await.unwrap().revision;
    let (release, gate) = std::sync::mpsc::channel();
    *files.publish_gate.lock().unwrap() = Some(gate);
    let editing = tokio::spawn({
        let settings = settings.clone();
        let revision = revision.clone();
        async move { settings.edit(initiator(), revision, save("a")).await }
    });
    within("the write starts", || {
        files.publishing.load(Ordering::SeqCst)
    })
    .await;
    editing.abort();
    assert!(editing.await.unwrap_err().is_cancelled());
    assert!(
        files.held.load(Ordering::SeqCst),
        "the lock was let go while the write was under way"
    );
    release.send(()).unwrap();
    within("the outcome is recorded", || audit.records().len() == 2).await;
    assert!(!files.held.load(Ordering::SeqCst), "never let go");
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["a", "nessa"]);
    assert!(matches!(
        outcome(&audit),
        McpServerOutcome::Applied {
            live_set_replaced: true,
            ..
        }
    ));
}

/// Shutdown during a save: it admits no more, and returns only once the
/// save under way has published, replaced the live set and recorded its
/// outcome.
#[tokio::test]
async fn shutdown_during_a_save_returns_after_its_outcome_is_recorded() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let settings = Arc::new(settings);
    let revision = settings.list().await.unwrap().revision;
    let (release, gate) = std::sync::mpsc::channel();
    *files.publish_gate.lock().unwrap() = Some(gate);
    let editing = tokio::spawn({
        let settings = settings.clone();
        let revision = revision.clone();
        async move { settings.edit(initiator(), revision, save("a")).await }
    });
    within("the write starts", || {
        files.publishing.load(Ordering::SeqCst)
    })
    .await;
    let stopping = tokio::spawn({
        let settings = settings.clone();
        async move { settings.shutdown().await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !stopping.is_finished(),
        "shutdown did not wait for the save"
    );
    assert_eq!(
        settings.edit(initiator(), revision, save("b")).await,
        Err(McpServerSettingsError::Stopping)
    );
    release.send(()).unwrap();
    assert_eq!(bounded(stopping).await, Ok(()));
    // Recorded before shutdown returned.
    assert!(matches!(
        outcome(&audit),
        McpServerOutcome::Applied {
            live_set_replaced: true,
            ..
        }
    ));
    assert_eq!(live(&servers), ["a", "nessa"]);
    assert!(bounded(editing).await.is_ok());
}

/// Shutdown during an inspection: the inspection is stopped, not waited
/// out — its gate is never let go — and shutdown returns once its outcome,
/// `cut: stopping` with no tools, is recorded; the caller is answered the
/// same.
#[tokio::test]
async fn shutdown_stops_a_running_inspection_and_records_it_cut() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files, audit.clone());
    let settings = Arc::new(settings);
    let held = inspector.gate.available_permits();
    inspector.gate.forget_permits(held);
    let inspecting = tokio::spawn({
        let settings = settings.clone();
        async move { settings.inspect(initiator(), "a").await }
    });
    within("the server is started", || {
        !inspector.asked.lock().unwrap().is_empty()
    })
    .await;
    let stopping = tokio::spawn({
        let settings = settings.clone();
        async move { settings.shutdown().await }
    });
    assert_eq!(bounded(stopping).await, Ok(()));
    // Recorded before shutdown returned.
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Inspected {
            tools: 0,
            cut: Some(InspectCut::Stopping),
        }
    );
    assert_eq!(
        bounded(inspecting).await,
        Ok(Inspection {
            tools: vec![],
            cut: Some(InspectCut::Stopping),
        })
    );
}

/// m7: an inspection shutdown ended — not started, or cut — is recorded
/// with cause `gateway_stopping` and the system as its initiator; its
/// requested record keeps the caller. One that ran its course — a deadline
/// included, which ends the caller's operation — stays the caller's.
#[tokio::test]
async fn shutdown_ended_inspections_are_recorded_as_the_gateway_stopping() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files, audit.clone());
    for (answer, cause) in [
        (
            Err(InspectFailure::Stopping),
            McpServerCause::GatewayStopping,
        ),
        (
            Ok(Inspection {
                tools: vec![],
                cut: Some(InspectCut::Stopping),
            }),
            McpServerCause::GatewayStopping,
        ),
        (
            Err(InspectFailure::TimedOut),
            McpServerCause::CallerRequested(initiator()),
        ),
        (
            Ok(Inspection {
                tools: vec![],
                cut: Some(InspectCut::Tools),
            }),
            McpServerCause::CallerRequested(initiator()),
        ),
    ] {
        audit.records.lock().unwrap().clear();
        *inspector.answer.lock().unwrap() = answer.clone();
        let _ = settings.inspect(initiator(), "a").await;
        outcome(&audit);
        let records = audit.records();
        assert_eq!(
            records[0].cause,
            McpServerCause::CallerRequested(initiator()),
            "{answer:?}"
        );
        assert_eq!(records[1].cause, cause, "{answer:?}");
    }
}

/// I-invalid, in the application: a stored server the SDK refuses to start
/// is answered with its problem and recorded `failed`, `invalid`, not
/// started — not `start_failed`.
#[tokio::test]
async fn an_invalid_stored_server_inspected_is_invalid_and_recorded_not_started() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files, audit.clone());
    let problem = ServerProblem::EnvironmentName {
        server: "a".into(),
        name: "1BAD".into(),
    };
    *inspector.answer.lock().unwrap() = Err(InspectFailure::Invalid(problem.clone()));
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::Inspect(InspectFailure::Invalid(
            problem
        )))
    );
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::InspectFailed {
            reason: "invalid",
            started: false,
        }
    );
}

/// The supervisors that run the gateway kill it 30 s after asking it to
/// stop: launchd's `ExitTimeOut` (`src-tauri/src/gateway/infrastructure/macos.rs`)
/// and systemd's `TimeoutStopSec` (`src-tauri/src/gateway/infrastructure/linux/unit.rs`),
/// both 30. Not reachable from this crate, so pinned here: raising either
/// does not loosen this.
const SUPERVISOR_STOP_WINDOW: Duration = Duration::from_secs(30);

/// The drain before the servers stop waits the store's lock wait and a
/// grace, not an inspection's deadline, and stays well inside the
/// supervisors' stop window, so the outcome records it waits for are not
/// lost to their kill — the rest of shutdown needs the remainder.
#[test]
fn the_drain_bound_is_inside_the_supervisors_stop_window() {
    let (settings, _) = settings_for(
        MemoryFiles::holding(config(vec![])),
        Arc::new(RecordingAudit::default()),
    );
    let bound = settings.drain_bound();
    assert_eq!(bound, super::drain_bound(LOCK_WAIT));
    assert!(bound < INSPECT_BOUNDS.deadline, "{bound:?}");
    assert!(bound <= SUPERVISOR_STOP_WINDOW / 4, "{bound:?}");
}

/// A live set whose `problem` or `replace` panics: before the publish, or
/// after it.
struct PanickingLive {
    live: crate::mcp_servers::infrastructure::LiveMcpServers,
    before_publish: bool,
}
impl super::LiveServerSet for PanickingLive {
    fn managed(&self) -> Option<ConfiguredMcpServer> {
        self.live.managed()
    }
    fn problem(&self, stored: &[ConfiguredMcpServer]) -> Option<ServerProblem> {
        assert!(!self.before_publish, "a fault before the publish");
        self.live.problem(stored)
    }
    fn replace(&self, _: &[ConfiguredMcpServer]) -> Result<(), super::LiveSetKept> {
        panic!("a fault after the publish")
    }
}

/// An inspector that panics before the server's launch begins, or once it
/// has.
struct PanickingInspector {
    launched: bool,
}
impl super::ServerInspector for PanickingInspector {
    fn inspect(
        &self,
        _: &ConfiguredMcpServer,
        _: super::InspectBounds,
        _: super::InspectStop,
        launch: super::LaunchBegun,
    ) -> super::InspectFuture<'_> {
        let launched = self.launched;
        Box::pin(async move {
            if launched {
                launch.mark();
                panic!("a fault once the server's launch began")
            }
            panic!("a fault before the server's launch began")
        })
    }
}

/// LS3i and S-panic-pre: a task that panics answers by how far it got.
/// After the publish — the file is new, its outcome unrecorded —
/// `audit_unavailable` with `applied: true`; before it,
/// `storage_unavailable` with `applied: false`, nothing written, and a
/// `failed` outcome recorded with reason `panicked`. An inspection is the
/// same about its server's launch. The lock and the admission it held are
/// let go either way.
#[tokio::test]
async fn a_panic_after_the_publish_answers_applied_and_before_it_storage_unavailable() {
    for before in [false, true] {
        let files = MemoryFiles::holding(config(vec![entry("a")]));
        let audit = Arc::new(RecordingAudit::default());
        let (settings, _) = super::super::infrastructure::settings_test_support::settings_through(
            files.clone(),
            audit.clone(),
            Arc::new(LeapingClock::default()),
            Arc::new(PanickingInspector { launched: !before }),
            |live| {
                Arc::new(PanickingLive {
                    live,
                    before_publish: before,
                })
            },
        );
        let revision = settings.list().await.unwrap().revision;
        let answered = settings.edit(initiator(), revision, save("b")).await;
        if before {
            assert_eq!(
                answered,
                Err(McpServerSettingsError::StorageUnavailable { applied: false })
            );
            assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
            assert_eq!(
                outcome(&audit),
                McpServerOutcome::Failed {
                    reason: "panicked",
                    before: None,
                }
            );
        } else {
            assert_eq!(
                answered,
                Err(McpServerSettingsError::AuditUnavailable {
                    applied: true,
                    cause: None,
                })
            );
            assert_eq!(stored(&files), ["a", "b"]);
            // Only the requested record: the outcome was never written.
            assert_eq!(audit.records().len(), 1);
        }
        assert!(
            !files.held.load(Ordering::SeqCst),
            "the lock was kept past the panic"
        );
        audit.records.lock().unwrap().clear();
        let inspected = settings.inspect(initiator(), "a").await;
        if before {
            assert_eq!(
                inspected,
                Err(McpServerSettingsError::StorageUnavailable { applied: false })
            );
            assert_eq!(
                outcome(&audit),
                McpServerOutcome::InspectFailed {
                    reason: "panicked",
                    started: false,
                }
            );
        } else {
            assert_eq!(
                inspected,
                Err(McpServerSettingsError::AuditUnavailable {
                    applied: true,
                    cause: None,
                })
            );
            assert_eq!(audit.records().len(), 1);
        }
        // Nothing still counted as running.
        assert_eq!(settings.shutdown().await, Ok(()));
    }
}

/// A panic before the publish whose `panicked` outcome cannot be recorded
/// either: `audit_unavailable`, not applied, with the storage failure as its
/// cause, so neither is lost.
#[tokio::test]
async fn a_panic_whose_outcome_cannot_be_recorded_keeps_both_causes() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, _) = super::super::infrastructure::settings_test_support::settings_through(
        files.clone(),
        audit.clone(),
        Arc::new(LeapingClock::default()),
        Arc::new(PanickingInspector { launched: false }),
        |live| {
            Arc::new(PanickingLive {
                live,
                before_publish: true,
            })
        },
    );
    let revision = settings.list().await.unwrap().revision;
    audit.fail_outcome.store(true, Ordering::SeqCst);
    let lost = McpServerSettingsError::AuditUnavailable {
        applied: false,
        cause: Some(Box::new(McpServerSettingsError::StorageUnavailable {
            applied: false,
        })),
    };
    assert_eq!(
        settings.edit(initiator(), revision, save("b")).await,
        Err(lost.clone())
    );
    assert_eq!(settings.inspect(initiator(), "a").await, Err(lost));
    assert_eq!(audit.records().len(), 2, "the two requested records");
}

/// Once shutdown has begun, a save, a remove or an inspection is not
/// admitted: `stopping`, with nothing locked, read, started or recorded.
#[tokio::test]
async fn a_request_after_shutdown_began_is_stopping_and_starts_nothing() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    let reads = files.reads.load(Ordering::SeqCst);
    assert_eq!(settings.shutdown().await, Ok(()));
    assert_eq!(
        settings
            .edit(initiator(), revision.clone(), save("b"))
            .await,
        Err(McpServerSettingsError::Stopping)
    );
    assert_eq!(
        settings.edit(initiator(), revision, remove("a")).await,
        Err(McpServerSettingsError::Stopping)
    );
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::Stopping)
    );
    assert_eq!(files.locks.load(Ordering::SeqCst), 0);
    assert_eq!(files.reads.load(Ordering::SeqCst), reads);
    assert!(inspector.asked.lock().unwrap().is_empty());
    assert!(audit.records().is_empty());
    // The list reads the file still.
    assert!(settings.list().await.is_ok());
}

/// Round 2, item 2: the requested record names the server asked for —
/// its executable and arguments among it — so a save refused before
/// anything is written still says what was asked to run.
#[tokio::test]
async fn a_refused_save_still_records_the_executable_it_asked_for() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, _) = settings_for(files, audit.clone());
    let asked = ServerEdit::Save(ServerSave {
        previous_name: None,
        server: StdioServer::new(
            "a",
            "/opt/tools/server",
            vec!["--serve".into(), "/data".into()],
        ),
        env: vec![("TOKEN".into(), Some("secret-value".into()))],
        enabled: false,
    });
    assert!(matches!(
        settings.edit(initiator(), "stale".into(), asked).await,
        Err(McpServerSettingsError::RevisionConflict { .. })
    ));
    let records = audit.records();
    assert_eq!(
        records[0].request.server,
        Some(Box::new(AuditedServer {
            name: "a".into(),
            command: "/opt/tools/server".into(),
            args: vec!["--serve".into(), "/data".into()],
            enabled: false,
            env_names: vec!["TOKEN".into()],
        }))
    );
    assert!(matches!(
        outcome(&audit),
        McpServerOutcome::Refused {
            reason: "revision_conflict",
            ..
        }
    ));
    assert!(!format!("{records:?}").contains("secret-value"));
    // A remove names no server it asks for.
    let revision = settings.list().await.unwrap().revision;
    let _ = settings.edit(initiator(), revision, remove("a")).await;
    assert_eq!(audit.records()[2].request.server, None);
}

/// On a gateway without the desktop, a `nessa` stored at startup is the
/// managed server, on or off: listed once, managed, with its own `enabled`
/// and variable names; counted towards the bound either way; launched only
/// when on.
#[tokio::test]
async fn a_stored_nessa_on_a_headless_gateway_is_the_managed_server_on_or_off() {
    for enabled in [true, false] {
        let mut stored_nessa = entry("nessa");
        stored_nessa["enabled"] = json!(enabled);
        stored_nessa["env"] = json!({"TOKEN": "secret"});
        let mut stored = vec![stored_nessa];
        stored.extend((0..MAX_MCP_SERVERS - 1).map(|index| entry(&format!("s{index}"))));
        let files = MemoryFiles::holding(config(stored));
        let startup = configured(server("nessa"), enabled, &[("TOKEN", "secret")]);
        let (settings, servers) =
            settings_started_with(files, Arc::new(RecordingAudit::default()), &[startup]);
        let list = settings.list().await.unwrap();
        assert_eq!(list.servers.len(), MAX_MCP_SERVERS, "{enabled}");
        let nessa: Vec<_> = list
            .servers
            .iter()
            .filter(|row| row.server.name() == "nessa")
            .collect();
        assert_eq!(nessa.len(), 1, "{enabled}");
        assert!(nessa[0].managed);
        assert_eq!(nessa[0].enabled, enabled);
        assert_eq!(nessa[0].env_names, ["TOKEN"]);
        assert!(!format!("{list:?}").contains("secret"));
        assert_eq!(live(&servers).contains(&"nessa".to_owned()), enabled);
        assert_eq!(
            settings
                .edit(initiator(), list.revision, save("one-more"))
                .await,
            Err(McpServerSettingsError::Invalid(EditProblem::Server(
                ServerProblem::TooMany
            ))),
            "{enabled}"
        );
    }
}

/// `LaunchSettings` names the managed server and the base environment's
/// variables, never a value.
#[test]
fn launch_settings_print_names_never_values() {
    let nessa = configured(managed().server().clone(), true, &[("M", "managed-secret")]);
    let base = BTreeMap::from([(OsString::from("HOME"), OsString::from("/home/secret-home"))]);
    let printed = format!("{:?}", LaunchSettings::new(&[nessa], "/w".into(), base));
    assert!(
        printed.contains("HOME") && printed.contains("nessa"),
        "{printed}"
    );
    assert!(!printed.contains("secret"), "{printed}");
}

/// A server's environment is the gateway's base with its own variables over
/// it: its value wins. The managed server is first and is never a stored
/// entry's.
#[test]
fn a_servers_own_variables_win_over_the_gateways() {
    let base = BTreeMap::from([
        (OsString::from("PATH"), OsString::from("/usr/bin")),
        (OsString::from("HOME"), OsString::from("/home/me")),
    ]);
    let launches = LaunchSettings::new(&[managed()], "/w".into(), base);
    let own = configured(server("a"), true, &[("PATH", "/mine")]);
    let managed_server = managed().server().clone();
    let stored_managed = configured(
        StdioServer::new(
            managed_server.name(),
            "/elsewhere",
            managed_server.args().to_vec(),
        ),
        true,
        &[],
    );
    let set = launches.launch_set(&[own, stored_managed]).unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(set[0].server, sdk_server(managed().server()));
    assert_eq!(set[1].environment[&OsString::from("PATH")], "/mine");
    assert_eq!(set[1].environment[&OsString::from("HOME")], "/home/me");
}

/// Settings over `files` inspecting with a scripted inspector, and both.
fn inspecting(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
) -> (super::McpServerSettings, Arc<ScriptedInspector>) {
    let inspector = Arc::new(ScriptedInspector::default());
    let (settings, _) = inspected_over(
        files,
        audit,
        Arc::new(LeapingClock::default()),
        inspector.clone(),
    );
    (settings, inspector)
}

/// Decision 8: an inspection takes a stored server — on or off — with its
/// variables, within the published bounds, and is recorded before the
/// server starts and after it stops: who asked, which server at which
/// revision, its variables' names and never their values, and what was read.
#[tokio::test]
async fn an_inspection_starts_a_stored_server_on_or_off_and_is_audited_both_sides() {
    let mut off = entry("off");
    off["enabled"] = json!(false);
    off["env"] = json!({"API_TOKEN": "secret-value"});
    let files = MemoryFiles::holding(config(vec![entry("on"), off]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    let read = Inspection {
        tools: vec![InspectedTool {
            name: "report".into(),
            read_only_hint: Some(true),
            destructive_hint: None,
            ui: None,
        }],
        cut: Some(InspectCut::Ui),
    };
    *inspector.answer.lock().unwrap() = Ok(read.clone());
    assert_eq!(settings.inspect(initiator(), "off").await, Ok(read));
    let asked = inspector.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0.server(), &server("off"));
    assert!(!asked[0].0.enabled());
    assert_eq!(asked[0].0.env()["API_TOKEN"], "secret-value");
    assert_eq!(asked[0].1, INSPECT_BOUNDS);
    let records = audit.records();
    assert_eq!(
        records[0].cause,
        McpServerCause::CallerRequested(initiator())
    );
    assert_eq!(records[1].cause, records[0].cause);
    assert_eq!(records[0].request.action, McpServerAction::Inspect);
    assert_eq!(records[0].request.target, "off");
    assert_eq!(records[0].request.revision, revision);
    assert_eq!(
        records[0].request.server,
        Some(audited("off", false, &["API_TOKEN"]))
    );
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Inspected {
            tools: 1,
            cut: Some(InspectCut::Ui),
        }
    );
    assert!(!format!("{records:?}").contains("secret-value"));
    // Nothing stored changes.
    assert_eq!(settings.list().await.unwrap().revision, revision);
}

/// I7, and what else starts nothing: an unknown name is `not_found`, the
/// managed server `reserved_name`; neither starts a server or is audited.
#[tokio::test]
async fn i7_an_unknown_or_managed_name_starts_nothing_and_records_nothing() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files, audit.clone());
    assert_eq!(
        settings.inspect(initiator(), "unknown").await,
        Err(McpServerSettingsError::NotFound)
    );
    assert_eq!(
        settings.inspect(initiator(), "nessa").await,
        Err(McpServerSettingsError::ReservedName)
    );
    assert!(inspector.asked.lock().unwrap().is_empty());
    assert!(audit.records().is_empty());
}

/// I6: two inspections run at once; a third is `busy` and starts nothing,
/// and a slot is free again once one ends.
#[tokio::test]
async fn i6_a_third_inspection_at_once_is_busy() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let (settings, inspector) = inspecting(files, Arc::new(RecordingAudit::default()));
    let settings = Arc::new(settings);
    inspector
        .gate
        .forget_permits(tokio::sync::Semaphore::MAX_PERMITS);
    let running: Vec<_> = (0..2)
        .map(|_| {
            let settings = settings.clone();
            tokio::spawn(async move { settings.inspect(initiator(), "a").await })
        })
        .collect();
    while inspector.asked.lock().unwrap().len() < 2 {
        tokio::task::yield_now().await;
    }
    // Answered at once: a third that started would wait at the gate.
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            settings.inspect(initiator(), "a")
        )
        .await
        .expect("a third inspection is answered at once"),
        Err(McpServerSettingsError::Busy)
    );
    assert_eq!(inspector.asked.lock().unwrap().len(), 2);
    inspector.gate.add_permits(2);
    for each in running {
        assert!(each.await.unwrap().is_ok());
    }
    assert!(settings.inspect(initiator(), "a").await.is_ok());
}

/// An inspection's audit: an unwritable first record starts nothing; the
/// server's failure is answered and recorded; an unwritable outcome says
/// whether the server was started and keeps the failure as its cause.
#[tokio::test]
async fn an_inspection_is_not_started_unaudited_and_keeps_both_causes() {
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, inspector) = inspecting(files, audit.clone());
    audit.fail_requested.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: false,
            cause: None,
        })
    );
    assert!(inspector.asked.lock().unwrap().is_empty());
    audit.fail_requested.store(false, Ordering::SeqCst);
    *inspector.answer.lock().unwrap() = Err(InspectFailure::TimedOut);
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::Inspect(InspectFailure::TimedOut))
    );
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::InspectFailed {
            reason: "timed_out",
            started: true,
        }
    );
    // Not started: said so, as `started: false`.
    audit.records.lock().unwrap().clear();
    *inspector.answer.lock().unwrap() = Err(InspectFailure::Stopping);
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::Inspect(InspectFailure::Stopping))
    );
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::InspectFailed {
            reason: "stopping",
            started: false,
        }
    );
    audit.fail_outcome.store(true, Ordering::SeqCst);
    for (failure, started) in [
        (InspectFailure::TimedOut, true),
        (InspectFailure::StartFailed, false),
        (InspectFailure::Stopping, false),
    ] {
        *inspector.answer.lock().unwrap() = Err(failure.clone());
        assert_eq!(
            settings.inspect(initiator(), "a").await,
            Err(McpServerSettingsError::AuditUnavailable {
                applied: started,
                cause: Some(Box::new(McpServerSettingsError::Inspect(failure))),
            })
        );
    }
    *inspector.answer.lock().unwrap() = Ok(Inspection {
        tools: vec![],
        cut: None,
    });
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::AuditUnavailable {
            applied: true,
            cause: None,
        })
    );
}
