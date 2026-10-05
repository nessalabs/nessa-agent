use super::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::desktop_runtime::application::ConversationData;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::desktop_runtime::domain::RetirementRefusal;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::desktop_runtime::infrastructure::ConversationDirectory;
use nessa_auth::{
    adapters::local::BootstrapRequest,
    application::dto::{
        CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
        OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
    },
};

/// A configuration naming both agents, with no file on disk for either.
///
/// Nothing here needs to exist: these are the paths the probe would stat, and
/// this is the code that decides which agents it gets to stat at all.
fn both_agents() -> AgentsConfig {
    let runtimes: serde_json::Map<String, serde_json::Value> = [AgentId::Claude, AgentId::Opencode]
        .into_iter()
        .map(|agent| {
            (
                agent.name().to_owned(),
                serde_json::json!({
                    "command": format!("/{}", agent.name()),
                    "args": [],
                    "model": "configured-model",
                    "toolsEnabled": true,
                }),
            )
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "catalog": "/catalog.json",
        "workspace": "/workspace",
        "selected": "claude",
        "runtimes": runtimes,
    }))
    .unwrap()
}

/// An agent the gateway could not start, with the agent already installed, is
/// not one the probe is asked about.
///
/// `providers` leaves an agent out for either of two reasons and only one of
/// them is a fact about this installation. A command nobody has installed yet
/// is a command somebody can install, and the probe re-stats it on every ask so
/// that setup keeps offering the button. A provider that could not be built
/// with everything already on disk failed on something no install re-asks, and
/// letting the probe stat that command would have it answer `ready` for an
/// agent no conversation can be opened on.
#[test]
fn an_agent_this_run_cannot_start_is_not_one_readiness_answers_for() {
    let config = both_agents();

    let all = launch_files(Some(&config), &HashSet::new());
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[&AgentId::Opencode].command,
        Path::new("/opencode"),
        "an agent nobody has installed is still the probe's to answer for"
    );

    let narrowed = launch_files(Some(&config), &HashSet::from([AgentId::Opencode]));
    assert!(!narrowed.contains_key(&AgentId::Opencode));
    assert!(
        narrowed.contains_key(&AgentId::Claude),
        "and the agents that did build are untouched"
    );
}

/// No agents configured is no agents to answer for, which is not the same
/// answer as an agent that is configured and unstartable — it is setup having
/// nothing to list.
#[test]
fn a_gateway_configured_with_no_agents_answers_for_none() {
    assert!(launch_files(None, &HashSet::new()).is_empty());
}

#[test]
fn agent_catalog_uses_binding_choices_for_each_catalog_model() {
    let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../nessa-sdk/data/models.json");
    let config: AgentsConfig = serde_json::from_value(serde_json::json!({
        "catalog": catalog,
        "workspace": "/workspace",
        "selected": "claude",
        "runtimes": {
            "claude": {"command": "/claude", "model": "claude-sonnet-5", "toolsEnabled": true},
            "codex": {"command": "/codex", "model": "gpt-6-astra", "toolsEnabled": true}
        }
    }))
    .unwrap();
    let offered = agent_catalog(
        &config,
        &HashSet::from([AgentId::Claude, AgentId::Codex]),
        None,
    )
    .unwrap();
    assert_eq!(offered.agents.len(), 2);
    for agent in &offered.agents {
        let selected = agent
            .models
            .iter()
            .find(|model| model.model_id == agent.default_model)
            .unwrap();
        assert_eq!(selected.approval_modes.len(), 3);
        assert_eq!(selected.approval_modes[0].id, WireApprovalMode::Ask);
        assert_eq!(selected.approval_modes[1].id, WireApprovalMode::Auto);
        assert_eq!(selected.approval_modes[2].id, WireApprovalMode::Full);
    }
    assert_eq!(offered.agents[0].agent, "claude");
    assert_eq!(offered.agents[1].agent, "codex");
    for agent in &offered.agents {
        for model in &agent.models {
            let ids: Vec<_> = model
                .approval_modes
                .iter()
                .map(|choice| choice.id)
                .collect();
            let expected = match model.model_id.as_str() {
                "claude-sonnet-5" | "gpt-6-astra" => vec![
                    WireApprovalMode::Ask,
                    WireApprovalMode::Auto,
                    WireApprovalMode::Full,
                ],
                _ if !cfg!(target_os = "macos") => vec![WireApprovalMode::Ask],
                "claude-haiku-4-5-20251001" => vec![WireApprovalMode::Ask, WireApprovalMode::Full],
                _ => vec![
                    WireApprovalMode::Ask,
                    WireApprovalMode::Auto,
                    WireApprovalMode::Full,
                ],
            };
            assert_eq!(ids, expected, "{} {}", agent.agent, model.model_id);
        }
    }
}

fn opened(result: Result<Arc<LocalReceiverAuthority>, RunError>) {
    if let Err(error) = result {
        panic!("journal did not open: {error}");
    }
}

fn namespace_with_registry() -> (tempfile::TempDir, std::path::PathBuf, LocalCredentialStore) {
    let directory = tempfile::tempdir().unwrap();
    let namespace = directory.path().join("namespace");
    nessa_local_storage::create_directory(&namespace.join("auth")).unwrap();
    let store = LocalCredentialStore::open(namespace.join("auth"), "credentials.json").unwrap();
    store
        .bootstrap(BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: OrganizationInputDto { id: "org".into() },
            principal: PrincipalInputDto {
                id: "owner".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member".into(),
                principal_id: "owner".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id: "credential".into(),
            issued_at: SystemClock.unix_seconds() - 1,
            expires_at: None,
            grants: ["credential.manage".into()]
                .into_iter()
                .map(|action| CredentialGrantDto {
                    action,
                    resource: ResourceDto {
                        organization_id: "org".into(),
                        id: "gateway".into(),
                    },
                })
                .collect(),
        })
        .unwrap();
    (directory, namespace, store)
}

/// Native pairing opens the receiver journal. That must not create
/// `conversations/`, which retirement reads as conversation data still there.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn a_native_only_journal_is_not_conversation_data() {
    let (_directory, namespace, store) = namespace_with_registry();
    assert!(!receiver_cleanup_owed(&store).unwrap());
    opened(receiver_access(&namespace, &store, "policy"));
    let journal = receiver_journal(&namespace);
    assert!(journal.is_file(), "{}", journal.display());
    assert!(!conversation_root(&namespace).exists());
    let data = ConversationDirectory::new(conversation_root(&namespace));
    assert!(data.missing());
    assert_eq!(
        RetirementRefusal::of(true, data.missing()),
        RetirementRefusal::DataMissing
    );
}

/// An absent journal is created when nothing owes a fence, and left absent
/// when an enrollment still does. Creating one would make that fence a
/// missing receiver.
#[test]
fn a_missing_journal_is_not_recreated_while_cleanup_is_owed() {
    let (_directory, namespace, store) = namespace_with_registry();
    opened(receiver_access(&namespace, &store, "policy"));
    let journal = receiver_journal(&namespace);
    std::fs::remove_file(&journal).unwrap();
    opened(receiver_access(&namespace, &store, "policy"));
    assert!(journal.is_file());

    std::fs::remove_file(&journal).unwrap();
    let Err(error) = open_receiver_journal(&namespace, "policy", true, &OsJournalFiles) else {
        panic!("a journal was opened while cleanup is owed");
    };
    let RunError::ReceiverJournal(missing) = &error else {
        panic!("expected a missing journal, got {error}");
    };
    assert_eq!(missing.path(), journal.as_path());
    assert!(error
        .to_string()
        .contains("receiver access journal missing"));
    assert!(!journal.exists());
    assert!(!conversation_root(&namespace).exists());
    assert!(
        matches!(
            open_receiver_journal(&namespace, "policy", true, &OsJournalFiles),
            Err(RunError::ReceiverJournal(_))
        ),
        "a second start created a journal"
    );
    assert!(!journal.exists());

    opened(open_receiver_journal(
        &namespace,
        "policy",
        false,
        &OsJournalFiles,
    ));
    opened(open_receiver_journal(
        &namespace,
        "policy",
        true,
        &OsJournalFiles,
    ));
    assert!(journal.is_file());
}

/// The owed-cleanup decision is read before any journal is created. A store
/// that cannot answer that read fails authentication and leaves the namespace
/// without a journal file.
#[test]
fn a_failed_pending_pairings_read_is_authentication_and_creates_no_journal() {
    let directory = tempfile::tempdir().unwrap();
    let namespace = directory.path().join("namespace");
    nessa_local_storage::create_directory(&namespace.join("auth")).unwrap();
    let store = LocalCredentialStore::open(namespace.join("auth"), "credentials.json").unwrap();
    assert!(!store.is_initialized());
    let Err(error) = receiver_access(&namespace, &store, "policy") else {
        panic!("a journal was opened without a pairing read");
    };
    assert!(matches!(error, RunError::Authentication(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("could not read whether receiver cleanup is owed"),
        "{error}"
    );
    assert!(!receiver_journal(&namespace).exists());
    assert!(!conversation_root(&namespace).exists());
}

/// A journal already stored under `conversations/` is moved once. The old
/// path is not what later opens. An empty conversation directory goes with
/// it; one that still holds another file stays.
#[test]
fn a_journal_left_under_conversations_is_moved() {
    let (_directory, namespace, _store) = namespace_with_registry();
    let legacy_dir = conversation_root(&namespace);
    nessa_local_storage::create_directory(&legacy_dir).unwrap();
    let legacy = legacy_dir.join("receiver-access.sqlite3");
    match LocalReceiverAuthority::open(&legacy, "policy", Arc::new(SystemClock)) {
        Ok(authority) => drop(authority),
        Err(error) => panic!("legacy journal did not open: {error}"),
    }
    // A killed write leaves the rollback journal, and a file left in
    // write-ahead mode leaves its log, beside the database.
    for suffix in ["-journal", "-wal", "-shm"] {
        std::fs::write(
            legacy_dir.join(format!("receiver-access.sqlite3{suffix}")),
            b"x",
        )
        .unwrap();
    }
    // Cleanup owed would refuse a missing journal. The move has to happen
    // first, or this start would refuse and leave the file where it was.
    opened(open_receiver_journal(
        &namespace,
        "policy",
        true,
        &OsJournalFiles,
    ));
    let journal = receiver_journal(&namespace);
    assert!(journal.is_file());
    assert!(!legacy.exists());
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(
            !legacy_dir
                .join(format!("receiver-access.sqlite3{suffix}"))
                .exists(),
            "{suffix} stayed under conversations/"
        );
    }
    assert!(!legacy_dir.exists());

    // An earlier start moved the database and was killed before the rollback
    // file. The next start still takes that file, and the directory goes.
    let (_partial, partial, _store) = namespace_with_registry();
    let partial_dir = conversation_root(&partial);
    nessa_local_storage::create_directory(&partial_dir).unwrap();
    let partial_legacy = partial_dir.join("receiver-access.sqlite3");
    match LocalReceiverAuthority::open(&partial_legacy, "policy", Arc::new(SystemClock)) {
        Ok(authority) => drop(authority),
        Err(error) => panic!("legacy journal did not open: {error}"),
    }
    let partial_journal = receiver_journal(&partial);
    nessa_local_storage::create_directory(partial_journal.parent().unwrap()).unwrap();
    std::fs::rename(&partial_legacy, &partial_journal).unwrap();
    std::fs::write(partial_dir.join("receiver-access.sqlite3-journal"), b"x").unwrap();
    opened(open_receiver_journal(
        &partial,
        "policy",
        true,
        &OsJournalFiles,
    ));
    assert!(!partial_dir.join("receiver-access.sqlite3-journal").exists());
    assert!(!partial_dir.exists());

    let (_kept, kept, _store) = namespace_with_registry();
    let kept_dir = conversation_root(&kept);
    nessa_local_storage::create_directory(&kept_dir).unwrap();
    let kept_journal = kept_dir.join("receiver-access.sqlite3");
    match LocalReceiverAuthority::open(&kept_journal, "policy", Arc::new(SystemClock)) {
        Ok(authority) => drop(authority),
        Err(error) => panic!("legacy journal did not open: {error}"),
    }
    std::fs::write(kept_dir.join("metadata.sqlite3"), b"keep").unwrap();
    opened(open_receiver_journal(
        &kept,
        "policy",
        true,
        &OsJournalFiles,
    ));
    assert!(receiver_journal(&kept).is_file());
    assert!(!kept_journal.exists());
    assert!(kept_dir.join("metadata.sqlite3").is_file());
}

/// Both journals on disk: the current file stays, and the old rollback is not
/// renamed onto it.
#[test]
fn both_journals_present_leaves_the_current_file() {
    let (_directory, namespace, _store) = namespace_with_registry();
    let current = receiver_journal(&namespace);
    nessa_local_storage::create_directory(current.parent().unwrap()).unwrap();
    std::fs::write(&current, b"current").unwrap();
    let legacy_dir = conversation_root(&namespace);
    nessa_local_storage::create_directory(&legacy_dir).unwrap();
    let legacy = legacy_dir.join("receiver-access.sqlite3");
    std::fs::write(&legacy, b"legacy").unwrap();
    std::fs::write(
        legacy_dir.join("receiver-access.sqlite3-journal"),
        b"old-journal",
    )
    .unwrap();
    adopt_legacy_journal(&namespace, &current, &OsJournalFiles).unwrap();
    assert_eq!(std::fs::read(&current).unwrap(), b"current");
    assert_eq!(std::fs::read(&legacy).unwrap(), b"legacy");
    assert_eq!(
        std::fs::read(legacy_dir.join("receiver-access.sqlite3-journal")).unwrap(),
        b"old-journal"
    );
}

/// A parent that is a file is not an absent journal.
#[test]
fn a_non_notfound_stat_is_agent_not_a_missing_journal() {
    let (_directory, namespace, _store) = namespace_with_registry();
    std::fs::write(namespace.join("receiver-access"), b"not-a-directory").unwrap();
    let error = match open_receiver_journal(&namespace, "policy", true, &OsJournalFiles) {
        Err(error) => error,
        Ok(_) => panic!("a file where the journal directory should be was opened"),
    };
    assert!(
        matches!(error, RunError::Agent(_)),
        "expected Agent, got {error}"
    );

    let (_directory, blocked, _store) = namespace_with_registry();
    std::fs::write(conversation_root(&blocked), b"not-a-directory").unwrap();
    let error = match open_receiver_journal(&blocked, "policy", true, &OsJournalFiles) {
        Err(error) => error,
        Ok(_) => panic!("a file where conversations/ should be was opened"),
    };
    assert!(
        matches!(error, RunError::Agent(_)),
        "expected Agent for the legacy path, got {error}"
    );
}

/// A symlink sidecar is not a regular file, so it stays and the directory stays.
#[test]
fn a_symlink_sidecar_is_left_and_keeps_conversations() {
    let (_directory, namespace, _store) = namespace_with_registry();
    let current = receiver_journal(&namespace);
    nessa_local_storage::create_directory(current.parent().unwrap()).unwrap();
    std::fs::write(&current, b"current").unwrap();
    let legacy_dir = conversation_root(&namespace);
    nessa_local_storage::create_directory(&legacy_dir).unwrap();
    let target = namespace.join("sidecar-target");
    std::fs::write(&target, b"linked").unwrap();
    let sidecar = legacy_dir.join("receiver-access.sqlite3-journal");
    std::os::unix::fs::symlink(&target, &sidecar).unwrap();
    std::fs::write(legacy_dir.join("receiver-access.sqlite3-wal"), b"wal").unwrap();
    adopt_legacy_journal(&namespace, &current, &OsJournalFiles).unwrap();
    assert!(sidecar.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"linked");
    assert!(legacy_dir.is_dir());
    assert!(!legacy_dir.join("receiver-access.sqlite3-wal").exists());
    assert_eq!(
        std::fs::read(
            current
                .parent()
                .unwrap()
                .join("receiver-access.sqlite3-wal")
        )
        .unwrap(),
        b"wal"
    );
    assert_eq!(std::fs::read(&current).unwrap(), b"current");
}

/// Names the substitute keeps. `Dir` is a directory; `File` is a regular file;
/// `Other` is anything a move must leave alone, a symlink included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemoryKind {
    Dir,
    File,
}

/// A journal filesystem with no disk. Moves, stats, and directory syncs are
/// the records this holds, so a test can fail the sync after the rename.
struct MemoryJournalFiles {
    entries: std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, MemoryKind>>,
    synced: std::sync::Mutex<Vec<std::path::PathBuf>>,
    fail_sync: bool,
}

impl MemoryJournalFiles {
    fn new(fail_sync: bool) -> Self {
        Self {
            entries: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            synced: std::sync::Mutex::new(Vec::new()),
            fail_sync,
        }
    }

    fn insert(&self, path: std::path::PathBuf, kind: MemoryKind) {
        self.entries.lock().unwrap().insert(path, kind);
    }

    fn kind(&self, path: &std::path::Path) -> Option<MemoryKind> {
        self.entries.lock().unwrap().get(path).copied()
    }

    fn synced(&self) -> Vec<std::path::PathBuf> {
        self.synced.lock().unwrap().clone()
    }
}

impl JournalFiles for MemoryJournalFiles {
    fn metadata(&self, path: &std::path::Path) -> std::io::Result<JournalEntry> {
        let entries = self.entries.lock().unwrap();
        let mut ancestor = path.parent();
        while let Some(directory) = ancestor {
            if entries.get(directory) == Some(&MemoryKind::File) {
                return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
            }
            ancestor = directory.parent();
        }
        match entries.get(path) {
            Some(MemoryKind::File) => Ok(JournalEntry::RegularFile),
            Some(_) => Ok(JournalEntry::Other),
            None => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        }
    }

    fn rename(&self, from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        let mut entries = self.entries.lock().unwrap();
        let kind = entries
            .remove(from)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
        entries.insert(to.to_path_buf(), kind);
        Ok(())
    }

    fn remove_dir(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut entries = self.entries.lock().unwrap();
        if entries.get(path) != Some(&MemoryKind::Dir) {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        if entries
            .keys()
            .any(|entry| entry.parent() == Some(path) && entry.as_path() != path)
        {
            return Err(std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty));
        }
        entries.remove(path);
        Ok(())
    }

    fn sync_directory(&self, path: &std::path::Path) -> std::io::Result<()> {
        if self.fail_sync {
            return Err(std::io::Error::other("directory sync failed"));
        }
        if self.entries.lock().unwrap().get(path) != Some(&MemoryKind::Dir) {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        self.synced.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }

    fn create_directory(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && entries.get(parent) != Some(&MemoryKind::Dir) {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
        }
        entries.insert(path.to_path_buf(), MemoryKind::Dir);
        Ok(())
    }
}

fn legacy_names(namespace: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let legacy_dir = conversation_root(namespace);
    let legacy = legacy_dir.join("receiver-access.sqlite3");
    (legacy_dir, legacy)
}

/// The move is not finished when the rename returns. The new directory, the
/// namespace, and the old directory are synced before the empty old directory
/// is removed, and the namespace is synced again once it is gone. Nothing
/// here is a real directory.
#[test]
fn a_legacy_move_syncs_its_directories_before_it_finishes() {
    let namespace = std::path::PathBuf::from("/namespace");
    let files = MemoryJournalFiles::new(false);
    files.insert(namespace.clone(), MemoryKind::Dir);
    let (legacy_dir, legacy) = legacy_names(&namespace);
    files.insert(legacy_dir.clone(), MemoryKind::Dir);
    files.insert(legacy.clone(), MemoryKind::File);
    files.insert(
        legacy_dir.join("receiver-access.sqlite3-journal"),
        MemoryKind::File,
    );
    let journal = receiver_journal(&namespace);
    adopt_legacy_journal(&namespace, &journal, &files).unwrap();
    assert_eq!(files.kind(&journal), Some(MemoryKind::File));
    assert_eq!(files.kind(&legacy), None);
    let sidecar = journal
        .parent()
        .unwrap()
        .join("receiver-access.sqlite3-journal");
    assert_eq!(files.kind(&sidecar), Some(MemoryKind::File));
    assert_eq!(files.kind(&legacy_dir), None);
    assert_eq!(
        files.synced(),
        vec![
            journal.parent().unwrap().to_path_buf(),
            namespace.clone(),
            legacy_dir,
            namespace,
        ]
    );
}

/// A file that is not a journal stays, so the old directory stays. The
/// namespace is still synced after the new directory and before the old one.
#[test]
fn a_legacy_move_syncs_the_namespace_when_the_old_directory_stays() {
    let namespace = std::path::PathBuf::from("/namespace");
    let files = MemoryJournalFiles::new(false);
    files.insert(namespace.clone(), MemoryKind::Dir);
    let (legacy_dir, legacy) = legacy_names(&namespace);
    files.insert(legacy_dir.clone(), MemoryKind::Dir);
    files.insert(legacy, MemoryKind::File);
    files.insert(legacy_dir.join("notes.txt"), MemoryKind::File);
    let journal = receiver_journal(&namespace);
    adopt_legacy_journal(&namespace, &journal, &files).unwrap();
    assert_eq!(files.kind(&journal), Some(MemoryKind::File));
    assert_eq!(files.kind(&legacy_dir), Some(MemoryKind::Dir));
    assert_eq!(
        files.synced(),
        vec![
            journal.parent().unwrap().to_path_buf(),
            namespace,
            legacy_dir,
        ]
    );
}

/// A directory sync that fails is the move not finishing. The names may
/// already have changed; startup does not report success or remove the old
/// directory.
#[test]
fn a_directory_sync_failure_leaves_the_move_unfinished() {
    let namespace = std::path::PathBuf::from("/namespace");
    let files = MemoryJournalFiles::new(true);
    files.insert(namespace.clone(), MemoryKind::Dir);
    let (legacy_dir, legacy) = legacy_names(&namespace);
    files.insert(legacy_dir.clone(), MemoryKind::Dir);
    files.insert(legacy, MemoryKind::File);
    let journal = receiver_journal(&namespace);
    let error = adopt_legacy_journal(&namespace, &journal, &files).unwrap_err();
    assert!(matches!(error, RunError::Agent(_)), "{error}");
    assert!(
        error.to_string().contains("directory sync failed"),
        "{error}"
    );
    assert_eq!(files.kind(&journal), Some(MemoryKind::File));
    assert_eq!(files.kind(&legacy_dir), Some(MemoryKind::Dir));
    assert!(files.synced().is_empty());
}
