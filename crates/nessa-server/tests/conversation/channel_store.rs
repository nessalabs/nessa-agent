//! Channels and their conversations survive a reopen; a retried creation
//! answers the kept channel; a conversation is in one channel for its life;
//! and a folder is checked, resolved, before agents may work in it.
use super::*;

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
    id[..request_id.len().min(10)].copy_from_slice(&request_id.as_bytes()[..request_id.len().min(10)]);
    Channel::new(
        ChannelId::from_random(id),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("owner").unwrap(),
        ChannelParts {
            name: ChannelName::new(name).unwrap(),
            folder: ChannelFolder {
                path: path("/work/repo"),
                made: false,
            },
            defaults: ChannelDefaults {
                agent: AgentId::Codex,
                model: ConversationModelId::new("gpt-5.6-terra").unwrap(),
                mode: ConversationApprovalMode::Full,
            },
            notes: ChannelNotes::new("line one\nline two").unwrap(),
            resources: ChannelResources::new(vec![path("/work/kit"), path("/work/a.json")])
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
    let (org, owner) = (OrganizationId::new("org").unwrap(), PrincipalId::new("owner").unwrap());
    let (first, second) = (made("request-1", "design", 2), made("request-2", "ops", 1));
    {
        let store = store(directory.path());
        assert_eq!(store.create(first.clone()).await.unwrap(), first);
        assert_eq!(store.create(second.clone()).await.unwrap(), second);
    }
    let store = LocalChannelStore::open(&directory.path().join("conversations/channels.sqlite3"))
        .unwrap();
    assert_eq!(store.list(&org, &owner).await.unwrap(), vec![first.clone(), second]);
    assert_eq!(store.get(first.id()).await.unwrap(), Some(first.clone()));
    assert_eq!(
        store.by_request(&org, &owner, "request-1").await.unwrap(),
        Some(first)
    );
    let stranger = PrincipalId::new("stranger").unwrap();
    assert!(store.list(&org, &stranger).await.unwrap().is_empty());
    assert_eq!(store.by_request(&org, &stranger, "request-1").await.unwrap(), None);
}

#[tokio::test]
async fn a_retried_creation_answers_the_channel_already_kept() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(directory.path());
    let kept = made("request-1", "design", 1);
    store.create(kept.clone()).await.unwrap();
    // Same request id, so the same id, but asked later with another name:
    // the kept one is answered, for the service to compare.
    assert_eq!(store.create(made("request-1", "other", 9)).await.unwrap(), kept);
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
    assert_eq!(store.channel_of(&placed).await.unwrap(), Some(design.id().clone()));
    assert_eq!(store.channel_of(&unplaced).await.unwrap(), None);
    let listed = store
        .channels_of(vec![placed.clone(), unplaced.clone()])
        .await
        .unwrap();
    assert_eq!(listed, HashMap::from([(placed.clone(), design.id().clone())]));
    store.forget(&placed).await.unwrap();
    assert_eq!(store.channel_of(&placed).await.unwrap(), None);
}

fn folders(directory: &Path) -> (LocalChannelFolders, PathBuf, PathBuf) {
    let data = directory.join("data");
    let home = directory.join("home");
    nessa_local_storage::create_directory(&data).unwrap();
    std::fs::create_dir_all(home.join("repo")).unwrap();
    std::fs::write(home.join("notes.md"), "notes").unwrap();
    let folders = LocalChannelFolders::new(&data, Some(home.clone())).unwrap();
    (
        folders,
        std::fs::canonicalize(data).unwrap(),
        std::fs::canonicalize(home).unwrap(),
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
    for refused in [Path::new("/"), &home, &data, Path::new("/etc"), Path::new("/usr/bin")] {
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
    assert_eq!(folders.make(&id).unwrap(), made, "making it again is no error");
    // Another channel may share a made folder, but not the channels folder.
    assert_eq!(folders.accept_folder(&made).unwrap(), made);
    assert_eq!(
        folders.accept_folder(&chosen(&data.join("channels"))),
        Err(FolderRefusal::Refused)
    );
    assert_eq!(
        folders.accept_resource(&chosen(&home.join("notes.md"))).unwrap(),
        chosen(&home.join("notes.md"))
    );
    assert_eq!(
        folders.accept_resource(&chosen(&home.join("gone.md"))),
        Err(FolderRefusal::NotFound)
    );
}
