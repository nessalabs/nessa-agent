//! Managing the stored MCP servers against the #391 PR 2 state table (rows
//! S2–S10 and S15–S17 here; S1 at the socket, S11–S14 and S18 with the live
//! set): each change audited before its lock and after its effect, written
//! as a whole file under the lock, and the live set replaced only after a
//! publish.
use super::{
    EditProblem, McpServerAction, McpServerAuditPhase, McpServerOutcome, McpServerSettingsError,
    ServerNames, ServerProblem,
};
use crate::mcp_servers::domain::{ConfiguredMcpServer, ServerEdit, ServerSave, StdioServer};
use crate::mcp_servers::infrastructure::settings_test_support::{
    config, entry, initiator, live, managed, server, settings_for, settings_over, MemoryFiles,
    RecordingAudit, UNPARSEABLE,
};
use crate::mcp_servers::infrastructure::{sdk_server, LaunchSettings};
use nessa_sdk::infrastructure::{
    acp::sessions::MAX_MCP_SERVERS, clock::RuntimeClock, mcp::MCP_SESSION_VARIABLE,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    sync::{atomic::Ordering, Arc},
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
    assert_eq!(records[0].initiator, initiator());
    assert_eq!(records[0].request.action, McpServerAction::Save);
    assert_eq!(records[0].request.target, "b");
    assert_eq!(records[0].request.revision, before.revision);
    assert_eq!(records[0].request.env_names, ["API_TOKEN"]);
    assert_eq!(
        outcome(&audit),
        McpServerOutcome::Applied {
            before: ServerNames {
                revision: before.revision,
                names: vec!["a".into()],
            },
            after: ServerNames {
                revision: after,
                names: vec!["a".into(), "b".into()],
            },
            live_set_replaced: true,
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
        .map(|row| (row.server.name.as_str(), row.enabled, row.managed))
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
            }),
        }
    );
}

/// S3: two saves at one revision at once are serialised by the lock; the
/// first wins and the second is refused with the first's revision.
#[tokio::test]
async fn s3_two_saves_at_one_revision_are_serialised_and_the_second_conflicts() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, _) = settings_over(files.clone(), audit, Arc::new(RuntimeClock::new()));
    let revision = settings.list().await.unwrap().revision;
    let (first, second) = tokio::join!(
        settings.edit(initiator(), revision.clone(), save("a")),
        settings.edit(initiator(), revision.clone(), save("b")),
    );
    let (won, lost) = match (first, second) {
        (Ok(won), Err(lost)) | (Err(lost), Ok(won)) => (won, lost),
        other => panic!("one save wins: {other:?}"),
    };
    assert_eq!(
        lost,
        McpServerSettingsError::RevisionConflict { revision: won }
    );
    assert_eq!(files.publishes.load(Ordering::SeqCst), 1);
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
        Err(McpServerSettingsError::StorageUnavailable)
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
        Err(McpServerSettingsError::AuditUnavailable { applied: false })
    );
    assert_eq!(files.locks.load(Ordering::SeqCst), 0);
    assert_eq!(files.publishes.load(Ordering::SeqCst), 0);
    assert_eq!(live(&servers), ["nessa"]);
    assert!(audit.records().is_empty());
}

/// S7: published, then the outcome record fails: `audit_unavailable` with
/// `applied`, and the file and live set are the new ones.
#[tokio::test]
async fn s7_an_unwritable_outcome_after_a_publish_says_it_applied() {
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let revision = settings.list().await.unwrap().revision;
    audit.fail_outcome.store(true, Ordering::SeqCst);
    assert_eq!(
        settings.edit(initiator(), revision, save("a")).await,
        Err(McpServerSettingsError::AuditUnavailable { applied: true })
    );
    assert_eq!(stored(&files), ["a"]);
    assert_eq!(live(&servers), ["a", "nessa"]);
    // A refusal whose outcome cannot be recorded says it did not apply.
    let revision = settings.list().await.unwrap().revision;
    assert_eq!(
        settings
            .edit(initiator(), revision, remove("unknown"))
            .await,
        Err(McpServerSettingsError::AuditUnavailable { applied: false })
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
    let mut refused = server("b");
    refused.args = vec![UNPARSEABLE.into()];
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
    let mut large = server("b");
    large.args = vec!["x".repeat(4096)];
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
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::Name)),
        ),
        (
            vec![entry("a"), entry("b")],
            save_with("b", Some("a"), vec![]),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::DuplicateName {
                name: "b".into(),
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
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::EnvironmentName)),
        ),
        (
            vec![],
            save_with("a", None, vec![(MCP_SESSION_VARIABLE, Some("token"))]),
            McpServerSettingsError::Invalid(EditProblem::Server(
                ServerProblem::ReservedEnvironmentName {
                    name: MCP_SESSION_VARIABLE.into(),
                },
            )),
        ),
        (
            vec![],
            save_with("a", None, vec![("KEY", Some("a\0b"))]),
            McpServerSettingsError::Invalid(EditProblem::Server(ServerProblem::EnvironmentValue {
                name: "KEY".into(),
            })),
        ),
        (
            vec![],
            save_with("a", None, vec![("KEY", Some("1")), ("KEY", Some("2"))]),
            McpServerSettingsError::Invalid(EditProblem::EnvironmentNameRepeated {
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
    let own = ConfiguredMcpServer {
        server: server("a"),
        enabled: true,
        env: BTreeMap::from([("PATH".to_owned(), "/mine".to_owned())]),
    };
    let stored_managed = ConfiguredMcpServer {
        server: StdioServer {
            command: "/elsewhere".into(),
            ..managed().server
        },
        ..managed()
    };
    let set = launches.launch_set(&[own, stored_managed]).unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(set[0].server, sdk_server(&managed().server));
    assert_eq!(set[1].environment[&OsString::from("PATH")], "/mine");
    assert_eq!(set[1].environment[&OsString::from("HOME")], "/home/me");
}
