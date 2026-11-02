//! Channels and their conversations survive a reopen; a retried creation
//! answers the kept channel; a conversation is in one channel for its life;
//! and a folder is checked, resolved, before agents may work in it.
use super::*;
use crate::conversation_test_support::absolute;

fn store(directory: &Path) -> LocalChannelStore {
    let private = directory.join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    LocalChannelStore::open(&private.join("channels.sqlite3")).unwrap()
}

fn conversation() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}

fn path(text: &str) -> ChannelPath {
    ChannelPath::new(PathBuf::from(text)).unwrap()
}

fn made(request_id: &str, name: &str, at: u64) -> Channel {
    let mut id = [0; 10];
    id[..request_id.len().min(10)]
        .copy_from_slice(&request_id.as_bytes()[..request_id.len().min(10)]);
    Channel::new(
        ChannelId::from_random(id),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("owner").unwrap(),
        ChannelParts {
            execution: crate::conversation::domain::ChannelExecution::default(),
            name: ChannelName::new(name).unwrap(),
            folder: ChannelFolder {
                path: path(&absolute("/work/repo")),
                made: false,
            },
            defaults: ChannelDefaults {
                agent: AgentId::Codex,
                model: ConversationModelId::new("gpt-5.6-terra").unwrap(),
                mode: ConversationApprovalMode::Full,
            },
            notes: ChannelNotes::new("line one\nline two").unwrap(),
            resources: ChannelResources::new(vec![
                path(&absolute("/work/kit")),
                path(&absolute("/work/a.json")),
            ])
            .unwrap(),
        },
        request_id.into(),
        at,
    )
    .unwrap()
}

#[tokio::test]
async fn channels_read_back_in_the_order_they_were_made_after_a_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let (org, owner) = (
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("owner").unwrap(),
    );
    let (first, second) = (made("request-1", "design", 2), made("request-2", "ops", 1));
    {
        let store = store(directory.path());
        assert_eq!(store.create(first.clone()).await.unwrap(), first);
        assert_eq!(store.create(second.clone()).await.unwrap(), second);
    }
    let store =
        LocalChannelStore::open(&directory.path().join("conversations/channels.sqlite3")).unwrap();
    assert_eq!(
        store.list(&org, &owner).await.unwrap(),
        vec![first.clone(), second]
    );
    assert_eq!(store.get(first.id()).await.unwrap(), Some(first.clone()));
    assert_eq!(
        store.by_request(&org, &owner, "request-1").await.unwrap(),
        Some(first)
    );
    let stranger = PrincipalId::new("stranger").unwrap();
    assert!(store.list(&org, &stranger).await.unwrap().is_empty());
    assert_eq!(
        store
            .by_request(&org, &stranger, "request-1")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_retried_creation_answers_the_channel_already_kept() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let kept = made("request-1", "design", 1);
    store.create(kept.clone()).await.unwrap();
    // Same request id, so the same id, but asked later with another name:
    // the kept one is answered, for the service to compare.
    assert_eq!(
        store.create(made("request-1", "other", 9)).await.unwrap(),
        kept
    );
}

#[tokio::test]
async fn a_conversation_stays_in_the_channel_it_was_placed_in() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let (design, ops) = (made("request-1", "design", 1), made("request-2", "ops", 2));
    store.create(design.clone()).await.unwrap();
    store.create(ops.clone()).await.unwrap();
    let (placed, unplaced) = (conversation(), conversation());
    store.place(&placed, design.id()).await.unwrap();
    store.place(&placed, design.id()).await.unwrap();
    assert!(matches!(
        store.place(&placed, ops.id()).await,
        Err(ConversationError::RequestConflict)
    ));
    assert_eq!(
        store.channel_of(&placed).await.unwrap(),
        Some(design.id().clone())
    );
    assert_eq!(store.channel_of(&unplaced).await.unwrap(), None);
    let listed = store
        .channels_of(vec![placed.clone(), unplaced.clone()])
        .await
        .unwrap();
    assert_eq!(
        listed,
        HashMap::from([(placed.clone(), design.id().clone())])
    );
    store.forget(&placed).await.unwrap();
    assert_eq!(store.channel_of(&placed).await.unwrap(), None);
}

fn folders(directory: &Path) -> (LocalChannelFolders, PathBuf, PathBuf) {
    let data = directory.join("data");
    let home = directory.join("home");
    nessa_local_storage::create_directory(&data).unwrap();
    std::fs::create_dir_all(home.join("repo")).unwrap();
    std::fs::create_dir_all(directory.join("outer").join("system").join("tools")).unwrap();
    std::fs::create_dir_all(directory.join("elsewhere")).unwrap();
    std::fs::write(home.join("notes.md"), "notes").unwrap();
    let folders = LocalChannelFolders::with_system(
        &data,
        Some(home.clone()),
        vec![dunce::canonicalize(directory.join("outer").join("system")).unwrap()],
    )
    .unwrap();
    (
        folders,
        dunce::canonicalize(data).unwrap(),
        dunce::canonicalize(home).unwrap(),
    )
}

fn chosen(path: &Path) -> ChannelPath {
    ChannelPath::new(path.to_path_buf()).unwrap()
}

#[test]
fn a_chosen_folder_must_exist_be_a_directory_and_not_be_refused() {
    let directory = tempfile::tempdir().unwrap();
    let (folders, data, home) = folders(directory.path());
    assert_eq!(
        folders.accept_folder(&chosen(&home.join("repo"))).unwrap(),
        chosen(&home.join("repo"))
    );
    assert_eq!(
        folders.accept_folder(&chosen(&home.join("missing"))),
        Err(FolderRefusal::NotFound)
    );
    assert_eq!(
        folders.accept_folder(&chosen(&home.join("notes.md"))),
        Err(FolderRefusal::NotADirectory)
    );
    // A folder beside the home folder is a place to work; one that holds it
    // or Nessa's data is not, nor is the root.
    let parent = dunce::canonicalize(directory.path()).unwrap();
    assert_eq!(
        folders
            .accept_folder(&chosen(&parent.join("elsewhere")))
            .unwrap(),
        chosen(&parent.join("elsewhere"))
    );
    let root = parent.ancestors().last().unwrap().to_path_buf();
    // `outer` holds a system folder, as `/private` holds `/etc`.
    let outer = parent.join("outer");
    let system = outer.join("system");
    for refused in [
        &root,
        &parent,
        &home,
        &data,
        &outer,
        &system,
        &system.join("tools"),
    ] {
        assert_eq!(
            folders.accept_folder(&chosen(refused)),
            Err(FolderRefusal::Refused),
            "{}",
            refused.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn a_folder_reached_through_a_link_is_kept_as_where_it_leads() {
    let directory = tempfile::tempdir().unwrap();
    let (folders, data, home) = folders(directory.path());
    std::os::unix::fs::symlink(home.join("repo"), home.join("link")).unwrap();
    assert_eq!(
        folders.accept_folder(&chosen(&home.join("link"))).unwrap(),
        chosen(&home.join("repo"))
    );
    // A link into Nessa's own data is refused as the data itself is.
    std::os::unix::fs::symlink(&data, home.join("into-data")).unwrap();
    assert_eq!(
        folders.accept_folder(&chosen(&home.join("into-data"))),
        Err(FolderRefusal::Refused)
    );
}

#[test]
fn a_made_folder_is_under_the_channels_folder_and_may_be_chosen_again() {
    let directory = tempfile::tempdir().unwrap();
    let (folders, data, home) = folders(directory.path());
    let id = ChannelId::from_random([3; 10]);
    let made = folders.make(&id).unwrap();
    assert_eq!(made, chosen(&data.join("channels").join(id.as_str())));
    assert!(made.as_path().is_dir());
    assert_eq!(
        folders.make(&id).unwrap(),
        made,
        "making it again is no error"
    );
    // Another channel may share a made folder, but not the channels folder.
    assert_eq!(folders.accept_folder(&made).unwrap(), made);
    assert_eq!(
        folders.accept_folder(&chosen(&data.join("channels"))),
        Err(FolderRefusal::Refused)
    );
    assert_eq!(
        folders
            .accept_resource(&chosen(&home.join("notes.md")))
            .unwrap(),
        chosen(&home.join("notes.md"))
    );
    assert_eq!(
        folders.accept_resource(&chosen(&home.join("gone.md"))),
        Err(FolderRefusal::NotFound)
    );
}

#[cfg(unix)]
#[test]
fn this_systems_folders_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("data");
    nessa_local_storage::create_directory(&data).unwrap();
    let folders = LocalChannelFolders::new(&data, None).unwrap();
    for refused in ["/etc", "/usr/bin"] {
        assert_eq!(
            folders.accept_folder(&chosen(Path::new(refused))),
            Err(FolderRefusal::Refused),
            "{refused}"
        );
    }
}

#[tokio::test]
async fn erasing_a_coordinator_rotates_once_and_survives_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let original = made("main-life", "design", 1);
    let first = original.coordinator_id();
    let worker = conversation();
    store.create(original.clone()).await.unwrap();
    store.place(&first, original.id()).await.unwrap();
    store.place(&worker, original.id()).await.unwrap();
    store.forget(&worker).await.unwrap();
    assert_eq!(
        store
            .get(original.id())
            .await
            .unwrap()
            .unwrap()
            .coordinator_id(),
        first
    );
    let (one, two) = tokio::join!(store.forget(&first), store.forget(&first));
    one.unwrap();
    two.unwrap();
    let next = store
        .get(original.id())
        .await
        .unwrap()
        .unwrap()
        .coordinator_id();
    assert_ne!(next, first);
    assert_eq!(store.channel_of(&first).await.unwrap(), None);
    // A retry of creation must answer the current generation, too.
    assert_eq!(
        store
            .create(original.clone())
            .await
            .unwrap()
            .coordinator_id(),
        next
    );
    drop(store);
    let reopened =
        LocalChannelStore::open(&directory.path().join("conversations/channels.sqlite3")).unwrap();
    let channel = reopened.get(original.id()).await.unwrap().unwrap();
    assert_eq!(channel.coordinator_id(), next);
    reopened.place(&next, channel.id()).await.unwrap();
    reopened.forget(&next).await.unwrap();
    let third = reopened
        .get(channel.id())
        .await
        .unwrap()
        .unwrap()
        .coordinator_id();
    assert_ne!(third, first);
    assert_ne!(third, next);
}

#[tokio::test]
async fn older_channel_schemas_upgrade_without_relocating_existing_work() {
    for version in [1, 2, 3] {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("channels.sqlite3");
        // Start from the actual old schema, rather than a second handwritten one.
        let current = DEFINITION;
        let old = current[..current.find("-- A replacement is recorded").unwrap()].to_owned();
        let old = if version == 3 {
            // Before a main agent could move: only an erasure retired one.
            current[..current.find("PRAGMA user_version").unwrap()].replace(
                "cause IN ('coordinator_conversation_erased', 'coordinator_moved')",
                "cause = 'coordinator_conversation_erased'",
            )
        } else if version == 1 {
            old.lines()
                .filter(|line| {
                    !line.trim_start().starts_with("coordinator_placement")
                        && !line.trim_start().starts_with("thread_placement")
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            old
        };
        drop(
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::OpenOrCreate).unwrap(),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!("{old}\nPRAGMA user_version = {version};"))
            .unwrap();
        let kept = made("migration", "old channel", 1);
        connection.execute("INSERT INTO channels (channel, organization_id, owner_id, request_id, name, folder, folder_made, agent, model, mode, notes, resources, created_at) VALUES (?1, 'org', 'owner', 'migration', 'old channel', ?2, 0, 'codex', 'gpt-5.6-terra', 'full', '', '[]', 1)", params![kept.id().as_str(), absolute("/work/repo")]).unwrap();
        let main = kept.coordinator_id();
        connection
            .execute(
                "INSERT INTO channel_conversations VALUES (?1, ?2)",
                params![main.to_string(), kept.id().as_str()],
            )
            .unwrap();
        drop(connection);
        let store = LocalChannelStore::open(&path).unwrap();
        let restored = store.get(kept.id()).await.unwrap().unwrap();
        assert_eq!(
            restored.execution(),
            crate::conversation::domain::ChannelExecution::default()
        );
        assert_eq!(restored.coordinator_id(), main);
        assert_eq!(
            store.channel_of(&main).await.unwrap(),
            Some(kept.id().clone())
        );
        store.forget(&main).await.unwrap();
        assert_ne!(
            store
                .get(kept.id())
                .await
                .unwrap()
                .unwrap()
                .coordinator_id(),
            main
        );
    }
}

#[tokio::test]
async fn moving_the_main_agent_names_a_fresh_main_chat_and_keeps_the_old_one_in_the_channel() {
    use crate::conversation::domain::{ChannelExecution, WorkspacePlacement};
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let original = made("moves", "design", 1);
    let first = original.coordinator_id();
    store.create(original.clone()).await.unwrap();
    store.place(&first, original.id()).await.unwrap();
    // Only new threads move: the main chat stays the same one.
    let threads = ChannelExecution {
        coordinator: WorkspacePlacement::Local,
        threads: WorkspacePlacement::Boat,
    };
    let changed = store
        .set_execution(original.id(), threads)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.execution(), threads);
    assert_eq!(changed.coordinator_id(), first);
    // The main agent moves: a fresh main chat, the old one still in the channel.
    let both = ChannelExecution {
        coordinator: WorkspacePlacement::Boat,
        threads: WorkspacePlacement::Boat,
    };
    let moved = store
        .set_execution(original.id(), both)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(moved.coordinator_id(), first);
    assert_eq!(
        store.channel_of(&first).await.unwrap(),
        Some(original.id().clone())
    );
    // Forgetting the old main chat, now a thread, retires nothing more.
    store.forget(&first).await.unwrap();
    drop(store);
    let reopened =
        LocalChannelStore::open(&directory.path().join("conversations/channels.sqlite3")).unwrap();
    let kept = reopened.get(original.id()).await.unwrap().unwrap();
    assert_eq!(kept.execution(), both);
    assert_eq!(kept.coordinator_id(), moved.coordinator_id());
    // Moving back names yet another one; no channel answers None.
    let back = reopened
        .set_execution(original.id(), ChannelExecution::default())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(back.coordinator_id(), moved.coordinator_id());
    assert_ne!(back.coordinator_id(), first);
    let missing = made("absent", "none", 1);
    assert!(reopened
        .set_execution(missing.id(), both)
        .await
        .unwrap()
        .is_none());
}
