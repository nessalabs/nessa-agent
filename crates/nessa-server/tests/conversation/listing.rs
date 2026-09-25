//! Listing reads ownership records, stored summaries, and live state, and opens
//! nothing; sending and completing a turn keep the summary current.
use super::{
    ConversationCaller, ConversationDependencies, ConversationError, ConversationLimits,
    ConversationListEntry, ConversationService, ProviderSessionErasers, SubmissionMode,
    SubmittedFile, SubmittedMessage, MAX_LISTED_CONVERSATIONS,
};
use crate::{
    agents::domain::AgentId,
    conversation::domain::{Conversation, ConversationId, ConversationSummary},
    conversation::{
        application::{
            ConversationListing, ConversationRepository, ConversationSummaries, ListedConversations,
        },
        infrastructure::LocalConversationStore,
    },
    conversation_test_support::{
        only, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryListing, MemoryRepository,
        MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit, TestClock,
        DELETION_BUDGETS,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection};
use nessa_sdk::{
    application::agent_execution::sessions::{SessionStorage, SessionStorageLease, StorageFuture},
    domain::agent_execution::sessions::SessionId,
    infrastructure::session_storage::InMemoryStorage,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::oneshot;

/// `TestClock`'s one reading, which every summary written here carries.
const NOW: u64 = 1_700_000_000_123;

/// Session storage that counts every lease asked of it, however it is asked.
#[derive(Default)]
struct CountingStorage {
    inner: InMemoryStorage,
    opens: AtomicUsize,
}
impl SessionStorage for CountingStorage {
    fn open(&self, id: SessionId) -> StorageFuture<'_, Box<dyn SessionStorageLease>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.inner.open(id)
    }
    fn open_existing(
        &self,
        id: SessionId,
    ) -> StorageFuture<'_, Option<Box<dyn SessionStorageLease>>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.inner.open_existing(id)
    }
}

struct Listing {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    repository: Arc<MemoryRepository>,
    summaries: Arc<MemorySummaries>,
}
fn listing(limits: ConversationLimits) -> Listing {
    let provider = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let summaries = Arc::new(MemorySummaries::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: Arc::new(CountingStorage::default()),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: summaries.clone(),
            listing: Arc::new(MemoryListing {
                repository: repository.clone(),
                summaries: summaries.clone(),
            }),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        limits,
        None,
    )
    .unwrap();
    Listing {
        service,
        provider,
        repository,
        summaries,
    }
}
/// A service over the store that keeps conversations for real, so what a
/// list shows — ownership, order, the bound, and whether it is whole — is the
/// store's answer and not a substitute's.
struct Stored {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    storage: Arc<CountingStorage>,
    store: Arc<LocalConversationStore>,
    path: std::path::PathBuf,
    _directory: tempfile::TempDir,
}
fn stored(limits: ConversationLimits) -> Stored {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let store = Arc::new(LocalConversationStore::open(&path).unwrap());
    let provider = Arc::new(ProviderFactory::default());
    let storage = Arc::new(CountingStorage::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: store.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: store.clone(),
            listing: store.clone(),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        limits,
        None,
    )
    .unwrap();
    Stored {
        service,
        provider,
        storage,
        store,
        path,
        _directory: directory,
    }
}
/// `count` conversations of `principal`'s in `organization`, each said in at
/// `at(n)` and archived when `archived`, written in one transaction through a
/// connection of the test's own. Returned in the order written.
fn stored_many(
    stored: &Stored,
    organization: &str,
    principal: &str,
    count: u64,
    archived: bool,
    at: impl Fn(u64) -> u64,
) -> Vec<ConversationId> {
    let mut raw = Connection::open(&stored.path).unwrap();
    let transaction = raw.transaction().unwrap();
    let written = (0..count)
        .map(|n| {
            let conversation = id();
            transaction
                .execute(
                    "INSERT INTO conversations VALUES (?1, ?2, ?3, 'panel', 'create', ?4, 'claude')",
                    params![conversation.to_string(), organization, principal, at(n) as i64],
                )
                .unwrap();
            transaction
                .execute(
                    "INSERT INTO summaries VALUES (?1, 'Said', 'said', ?2, ?3)",
                    params![conversation.to_string(), at(n) as i64, archived],
                )
                .unwrap();
            conversation
        })
        .collect();
    transaction.commit().unwrap();
    written
}
async fn stored_put(
    stored: &Stored,
    id: &ConversationId,
    organization: &str,
    principal: &str,
    at: u64,
) {
    let record = Conversation::new(
        id.clone(),
        OrganizationId::new(organization).unwrap(),
        PrincipalId::new(principal).unwrap(),
        "panel".into(),
        "create".into(),
        at,
        AgentId::Claude,
    )
    .unwrap();
    ConversationRepository::create(stored.store.as_ref(), record)
        .await
        .unwrap();
}
async fn stored_say(stored: &Stored, id: &ConversationId, text: &str, at: u64) {
    ConversationSummaries::record(
        stored.store.as_ref(),
        id,
        ConversationSummary::after_message(None, text, None, at),
    )
    .await
    .unwrap();
}
/// Change one stored row the way only a hand edit could.
fn damage(stored: &Stored, statement: &str, id: &ConversationId) {
    Connection::open(&stored.path)
        .unwrap()
        .execute(statement, [id.to_string()])
        .unwrap();
}
fn id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}
fn caller(organization: &str, principal: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new(organization).unwrap(),
        principal_id: PrincipalId::new(principal).unwrap(),
        surface_id: "panel".into(),
        action_id: "list".into(),
    }
}
fn owner() -> ConversationCaller {
    caller("org", "person")
}
fn put(listing: &Listing, id: &ConversationId, organization: &str, principal: &str, at: u64) {
    let record = Conversation::new(
        id.clone(),
        OrganizationId::new(organization).unwrap(),
        PrincipalId::new(principal).unwrap(),
        "panel".into(),
        "create".into(),
        at,
        AgentId::Claude,
    )
    .unwrap();
    listing
        .repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), record);
}
fn summarize(listing: &Listing, id: &ConversationId, text: &str, at: u64) {
    listing.summaries.summaries.lock().unwrap().insert(
        id.clone(),
        ConversationSummary::after_message(None, text, None, at),
    );
}
fn ids(entries: &[ConversationListEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry.conversation_id.clone())
        .collect()
}
fn message(text: &str) -> SubmittedMessage {
    SubmittedMessage {
        text: text.into(),
        ..SubmittedMessage::default()
    }
}
/// The owner's list, once `ready` holds of it.
async fn listed_when(
    service: &ConversationService,
    ready: impl Fn(&[ConversationListEntry]) -> bool,
) -> Vec<ConversationListEntry> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let listed = service.list(owner(), false).await.unwrap().conversations;
            if ready(&listed) {
                return listed;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the list reaches the expected state")
}

#[tokio::test]
async fn listing_shows_only_the_callers_conversations_and_opens_nothing() {
    // Room for one live conversation, so a list that opened or reserved one
    // would leave no room for the creation at the end.
    let listing = stored(ConversationLimits {
        max_conversations: 1,
        ..ConversationLimits::default()
    });
    let mine = id();
    let another_principal = id();
    let another_organization = id();
    let another_by_case = id();
    stored_put(&listing, &mine, "org", "person", 10).await;
    stored_say(&listing, &mine, "mine", 15).await;
    stored_put(&listing, &another_principal, "org", "other", 20).await;
    stored_say(&listing, &another_principal, "not yours", 40).await;
    stored_put(&listing, &another_organization, "elsewhere", "person", 30).await;
    stored_say(&listing, &another_organization, "not yours either", 50).await;
    // Ownership is the owner's text exactly: one letter's case is somebody
    // else.
    stored_put(&listing, &another_by_case, "org", "Person", 30).await;
    stored_say(&listing, &another_by_case, "nor this", 60).await;

    let listed = listing.service.list(owner(), false).await.unwrap();
    assert!(listed.complete);
    assert_eq!(
        listed.conversations,
        [ConversationListEntry {
            conversation_id: mine.to_string(),
            title: Some("mine".into()),
            preview: Some("mine".into()),
            created_at_ms: 10,
            updated_at_ms: 15,
            running: false,
            archived: false,
        }]
    );
    for ((organization, principal), expected) in [
        (("org", "other"), &another_principal),
        (("elsewhere", "person"), &another_organization),
        (("org", "Person"), &another_by_case),
    ] {
        assert_eq!(
            ids(&listing
                .service
                .list(caller(organization, principal), false)
                .await
                .unwrap()
                .conversations),
            [expected.to_string()]
        );
    }
    assert!(listing
        .service
        .list(caller("nobody", "nobody"), false)
        .await
        .unwrap()
        .conversations
        .is_empty());
    // No provider opened, and no session storage asked for a lease.
    assert_eq!(listing.provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(listing.storage.opens.load(Ordering::SeqCst), 0);

    // Listing took no live slot: the one slot there is still opens a new one.
    let created = id();
    listing
        .service
        .create(created.clone(), owner(), None)
        .await
        .unwrap();
    stored_say(&listing, &created, "live", 60).await;
    assert_eq!(listing.provider.open_calls.load(Ordering::SeqCst), 1);
    let opens = listing.storage.opens.load(Ordering::SeqCst);
    // And a live conversation is listed without being opened again, or its
    // storage asked for again.
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(listed.len(), 2);
    assert_eq!(listing.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(listing.storage.opens.load(Ordering::SeqCst), opens);
    listing.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_conversation_nothing_was_said_in_is_not_listed() {
    let listing = listing(ConversationLimits::default());
    // Opened only to hold an upload, say, and never written in.
    let quiet = id();
    put(&listing, &quiet, "org", "person", 10);
    for archived in [false, true] {
        assert!(listing
            .service
            .list(owner(), archived)
            .await
            .unwrap()
            .conversations
            .is_empty());
    }
    // Once something is said, it is.
    summarize(&listing, &quiet, "hello", 20);
    assert_eq!(
        ids(&listing
            .service
            .list(owner(), false)
            .await
            .unwrap()
            .conversations),
        [quiet.to_string()]
    );
}

#[tokio::test]
async fn archiving_a_conversation_nothing_was_said_in_changes_nothing() {
    let listing = listing(ConversationLimits::default());
    let quiet = id();
    put(&listing, &quiet, "org", "person", 30);
    // Not applied, and nothing is made: it stays out of both lists, before an
    // unarchive and after it.
    for archived in [true, false] {
        assert!(!listing
            .service
            .archive(quiet.clone(), owner(), archived)
            .await
            .unwrap());
        for listed in [false, true] {
            assert!(listing
                .service
                .list(owner(), listed)
                .await
                .unwrap()
                .conversations
                .is_empty());
        }
    }
    assert!(listing.summaries.summaries.lock().unwrap().is_empty());
    assert_eq!(listing.summaries.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_read_carries_the_title_the_list_shows() {
    let listing = listing(ConversationLimits::default());
    let conversation = id();
    listing
        .service
        .create(conversation.clone(), owner(), None)
        .await
        .unwrap();
    // Nothing said yet: no title, in either.
    let view = listing
        .service
        .read(conversation.clone(), owner())
        .await
        .unwrap();
    assert_eq!(view.title, None);
    listing
        .service
        .submit(
            conversation.clone(),
            owner(),
            "first".into(),
            message("**Plan** the 3-day trip\nand more"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    let view = listing
        .service
        .read(conversation.clone(), owner())
        .await
        .unwrap();
    assert_eq!(view.title.as_deref(), Some("Plan the 3-day trip"));
    assert_eq!(view.title, listed[0].title);
    // One rule: change the stored title and both follow it.
    listing.summaries.summaries.lock().unwrap().insert(
        conversation.clone(),
        ConversationSummary::after_message(None, "Renamed", None, NOW),
    );
    let view = listing
        .service
        .read(conversation.clone(), owner())
        .await
        .unwrap();
    assert_eq!(view.title.as_deref(), Some("Renamed"));
    assert_eq!(
        listing
            .service
            .list(owner(), false)
            .await
            .unwrap()
            .conversations[0]
            .title,
        view.title
    );
    // A summary that cannot be read costs the view its title, not the read.
    listing.summaries.load_fails.store(true, Ordering::SeqCst);
    let view = listing
        .service
        .read(conversation.clone(), owner())
        .await
        .unwrap();
    assert_eq!(view.title, None);
    listing.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn listing_refuses_a_caller_whose_action_cannot_be_recorded_and_after_retirement() {
    let listing = listing(ConversationLimits::default());
    let mut invalid = owner();
    invalid.action_id = "line\nbreak".into();
    assert!(matches!(
        listing.service.list(invalid, false).await,
        Err(ConversationError::InvalidInput)
    ));
    listing.service.shutdown().await.unwrap();
    assert!(matches!(
        listing.service.list(owner(), false).await,
        Err(ConversationError::Unavailable)
    ));
}

#[tokio::test]
async fn the_bound_is_applied_after_ownership_and_keeps_the_newest() {
    let listing = stored(ConversationLimits::default());
    let bound = MAX_LISTED_CONVERSATIONS as u64;
    // More than a whole list of somebody else's conversations, every one of
    // them newer than anything the caller has.
    stored_many(&listing, "org", "other", bound + 100, false, |n| 10_000 + n);
    let mine = stored_many(&listing, "org", "person", 3, false, |n| n);
    let listed = listing.service.list(owner(), false).await.unwrap();
    assert!(listed.complete);
    assert_eq!(
        ids(&listed.conversations),
        mine.iter()
            .rev()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );

    // Past the bound, the oldest of the caller's own are the ones left out.
    let mut own = mine;
    own.extend(stored_many(
        &listing,
        "org",
        "person",
        bound + 7,
        false,
        |n| 3 + n,
    ));
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(listed.len(), MAX_LISTED_CONVERSATIONS);
    assert_eq!(
        ids(&listed),
        own.iter()
            .rev()
            .take(MAX_LISTED_CONVERSATIONS)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert!(listed
        .windows(2)
        .all(|pair| pair[0].updated_at_ms > pair[1].updated_at_ms));
}

#[tokio::test]
async fn newest_summary_first_then_identity_and_an_unreadable_row_is_counted_not_shown() {
    let listing = stored(ConversationLimits::default());
    let [older, tied_low, tied_high] = {
        let mut three = [id(), id(), id()];
        three[1..].sort_by_key(ToString::to_string);
        three
    };
    stored_put(&listing, &older, "org", "person", 50).await;
    stored_say(&listing, &older, "older", 50).await;
    stored_put(&listing, &tied_high, "org", "person", 2).await;
    stored_put(&listing, &tied_low, "org", "person", 1).await;
    stored_say(&listing, &tied_high, "high", 100).await;
    stored_say(&listing, &tied_low, "low", 100).await;
    let listed = listing.service.list(owner(), false).await.unwrap();
    assert!(listed.complete);
    assert_eq!(
        ids(&listed.conversations),
        [
            tied_low.to_string(),
            tied_high.to_string(),
            older.to_string()
        ]
    );
    let first = &listed.conversations[0];
    assert_eq!(first.title.as_deref(), Some("low"));
    assert_eq!(first.preview.as_deref(), Some("low"));
    assert_eq!(first.created_at_ms, 1);
    assert_eq!(first.updated_at_ms, 100);

    // A summary, or a record, that cannot be read back is left out of the
    // list its stored flag files it under, and that list says it is not
    // whole. The other list never met it and is.
    for statement in [
        "UPDATE summaries SET title = '' WHERE conversation_id = ?1",
        "UPDATE conversations SET creator_surface = '' WHERE id = ?1",
    ] {
        let raw = Connection::open(&listing.path).unwrap();
        raw.execute(statement, [tied_low.to_string()]).unwrap();
        let listed = listing.service.list(owner(), false).await.unwrap();
        assert_eq!(
            ids(&listed.conversations),
            [tied_high.to_string(), older.to_string()],
            "{statement}"
        );
        assert!(!listed.complete, "{statement}");
        assert!(listing.service.list(owner(), true).await.unwrap().complete);
        // Repaired, it is back, and the list is whole again.
        raw.execute(
            "UPDATE summaries SET title = 'low' WHERE conversation_id = ?1",
            [tied_low.to_string()],
        )
        .unwrap();
        raw.execute(
            "UPDATE conversations SET creator_surface = 'panel' WHERE id = ?1",
            [tied_low.to_string()],
        )
        .unwrap();
        assert!(listing.service.list(owner(), false).await.unwrap().complete);
    }
}

#[tokio::test]
async fn sending_titles_a_conversation_once_and_previews_what_was_said_last() {
    let listing = listing(ConversationLimits::default());
    let conversation = id();
    listing
        .service
        .create(conversation.clone(), owner(), None)
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *listing.provider.execution_gate.lock().unwrap() = Some(gate);
    listing
        .service
        .submit(
            conversation.clone(),
            caller("org", "person"),
            "first".into(),
            message("  Plan the **trip**\nsecond line"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        listing.provider.execution_started.notified(),
    )
    .await
    .unwrap();
    // Accepted and running: the list shows the message, not yet a reply.
    let listed = listed_when(&listing.service, |listed| listed[0].running).await;
    assert_eq!(listed[0].title.as_deref(), Some("Plan the trip"));
    assert_eq!(
        listed[0].preview.as_deref(),
        Some("Plan the trip second line")
    );
    assert_eq!(listed[0].updated_at_ms, NOW);
    let writes = listing.summaries.writes.load(Ordering::SeqCst);
    assert_eq!(writes, 1);

    // A retry of the same submission says nothing new.
    listing
        .service
        .submit(
            conversation.clone(),
            caller("org", "person"),
            "first".into(),
            message("  Plan the **trip**\nsecond line"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(listing.summaries.writes.load(Ordering::SeqCst), writes);

    // Completed: the reply is the last thing said.
    release.send(()).unwrap();
    let listed = listed_when(&listing.service, |listed| {
        listed[0].preview.as_deref() == Some("Response: Plan the trip second line")
    })
    .await;
    assert!(!listed[0].running);
    assert_eq!(listed[0].title.as_deref(), Some("Plan the trip"));

    // A later message never renames the conversation.
    listing
        .service
        .submit(
            conversation.clone(),
            caller("org", "person"),
            "second".into(),
            message("Something else"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let listed = listed_when(&listing.service, |listed| {
        listed[0].preview.as_deref() == Some("Response: Something else")
    })
    .await;
    assert_eq!(listed[0].title.as_deref(), Some("Plan the trip"));
    assert!(!listed[0].running);
    listing.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_message_of_files_alone_is_titled_by_its_first_file() {
    let listing = listing(ConversationLimits::default());
    let conversation = id();
    listing
        .service
        .create(conversation.clone(), owner(), None)
        .await
        .unwrap();
    let (release, gate) = oneshot::channel::<()>();
    *listing.provider.execution_gate.lock().unwrap() = Some(gate);
    listing
        .service
        .submit(
            conversation.clone(),
            owner(),
            "files".into(),
            SubmittedMessage {
                text: " ".into(),
                images: Vec::new(),
                files: vec![
                    SubmittedFile {
                        path: "/Users/ada/report.pdf".into(),
                    },
                    SubmittedFile {
                        path: "/Users/ada/other.pdf".into(),
                    },
                ],
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(listed[0].title.as_deref(), Some("report.pdf"));
    assert_eq!(listed[0].preview.as_deref(), Some("Attachment"));
    drop(release);
    listing.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_summary_that_cannot_be_written_does_not_fail_the_message() {
    let listing = listing(ConversationLimits::default());
    listing.summaries.record_fails.store(true, Ordering::SeqCst);
    let conversation = id();
    listing
        .service
        .create(conversation.clone(), owner(), None)
        .await
        .unwrap();
    listing
        .service
        .submit(
            conversation.clone(),
            owner(),
            "first".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .expect("the message is accepted although its summary was not written");
    // The turn still completes, and its reply's write is attempted and fails too.
    tokio::time::timeout(Duration::from_secs(3), async {
        while listing.summaries.writes.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let view = listing
        .service
        .read(conversation.clone(), owner())
        .await
        .unwrap();
    assert_eq!(view.messages.len(), 1);
    // Nothing of what was said could be kept, so the list has nothing to
    // show for it: stale, as a summary that cannot be written leaves it, and
    // the conversation itself is untouched.
    assert!(listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations
        .is_empty());

    // A summary that cannot be read is left alone, and the message still goes.
    listing
        .summaries
        .record_fails
        .store(false, Ordering::SeqCst);
    listing.summaries.load_fails.store(true, Ordering::SeqCst);
    let writes = listing.summaries.writes.load(Ordering::SeqCst);
    listing
        .service
        .submit(
            conversation.clone(),
            owner(),
            "second".into(),
            message("again"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(listing.summaries.writes.load(Ordering::SeqCst), writes);
    listing.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn archiving_moves_a_conversation_between_the_lists_and_a_new_message_brings_it_back() {
    let listing = listing(ConversationLimits::default());
    let conversation = id();
    listing
        .service
        .create(conversation.clone(), owner(), None)
        .await
        .unwrap();
    let service = &listing.service;
    // Nothing said yet: it is in neither list, and archiving or unarchiving it
    // changes nothing and makes nothing to list.
    for archived in [true, false] {
        assert!(!service
            .archive(conversation.clone(), owner(), archived)
            .await
            .unwrap());
    }
    assert!(listing.summaries.summaries.lock().unwrap().is_empty());
    service
        .submit(
            conversation.clone(),
            owner(),
            "first".into(),
            message("hello"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    // Unarchiving what is not archived changes nothing, and says so.
    assert!(!service
        .archive(conversation.clone(), owner(), false)
        .await
        .unwrap());
    assert!(service
        .archive(conversation.clone(), owner(), true)
        .await
        .unwrap());
    // Repeated, including by the same request: already archived.
    assert!(!service
        .archive(conversation.clone(), owner(), true)
        .await
        .unwrap());
    assert!(service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations
        .is_empty());
    let archived = service.list(owner(), true).await.unwrap().conversations;
    assert_eq!(ids(&archived), [conversation.to_string()]);
    assert!(archived[0].archived);
    // Archiving stopped nothing and opened nothing.
    assert_eq!(listing.provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(listing.provider.close_calls.load(Ordering::SeqCst), 0);

    // A message in an archived conversation unarchives it.
    service
        .submit(
            conversation.clone(),
            owner(),
            "second".into(),
            message("again"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let listed = listed_when(service, |listed| {
        listed.first().is_some_and(|entry| !entry.archived)
    })
    .await;
    assert_eq!(ids(&listed), [conversation.to_string()]);
    assert!(!listed[0].archived);
    assert!(service
        .list(owner(), true)
        .await
        .unwrap()
        .conversations
        .is_empty());

    // Its reply, settling after a later archive, leaves it archived.
    assert!(service
        .archive(conversation.clone(), owner(), true)
        .await
        .unwrap());
    let summary = listing.summaries.summaries.lock().unwrap()[&conversation].clone();
    assert!(summary.archived());
    assert_eq!(summary.title().unwrap().as_str(), "hello");
    // Nobody else can archive it, and there is nothing to archive where
    // nothing was created.
    for (target, who) in [
        (conversation.clone(), caller("org", "other")),
        (id(), owner()),
    ] {
        assert!(matches!(
            service.archive(target, who, true).await,
            Err(ConversationError::NotFound)
        ));
    }
    listing.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_archive_that_cannot_be_written_fails_visibly() {
    let listing = listing(ConversationLimits::default());
    let conversation = id();
    put(&listing, &conversation, "org", "person", 1);
    summarize(&listing, &conversation, "hello", 1);
    // Unlike the rest of a summary, this is somebody's decision: a failure to
    // keep it is the command's failure, not a line in a log.
    listing.summaries.record_fails.store(true, Ordering::SeqCst);
    assert!(matches!(
        listing
            .service
            .archive(conversation.clone(), owner(), true)
            .await,
        Err(ConversationError::Metadata)
    ));
    assert!(ids(&listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations)
    .contains(&conversation.to_string()));
    // Nor is an archive decided on a summary that could not be read.
    listing
        .summaries
        .record_fails
        .store(false, Ordering::SeqCst);
    listing.summaries.load_fails.store(true, Ordering::SeqCst);
    let writes = listing.summaries.writes.load(Ordering::SeqCst);
    assert!(matches!(
        listing
            .service
            .archive(conversation.clone(), owner(), true)
            .await,
        Err(ConversationError::Metadata)
    ));
    assert_eq!(listing.summaries.writes.load(Ordering::SeqCst), writes);
}

#[tokio::test]
async fn a_list_says_whether_the_bound_left_any_out() {
    let listing = stored(ConversationLimits::default());
    let bound = MAX_LISTED_CONVERSATIONS as u64;
    // Another principal's conversations, and the caller's own that nothing
    // was said in, are not the caller's to list and never make it incomplete.
    stored_many(&listing, "org", "other", bound + 50, false, |n| n);
    for n in 0..10 {
        stored_put(&listing, &id(), "org", "person", n).await;
    }
    stored_many(&listing, "org", "person", bound - 1, false, |n| 1_000 + n);
    let under = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(under.conversations.len(), MAX_LISTED_CONVERSATIONS - 1);
    assert!(under.complete);
    // Exactly at the bound, with nothing behind it: all of them.
    stored_many(&listing, "org", "person", 1, false, |_| 5_000);
    let at = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(at.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(at.complete);
    // One more behind the bound: the list is cut, and says so.
    stored_many(&listing, "org", "person", 1, false, |_| 5_001);
    let past = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(past.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(!past.complete);

    // The archived list is judged by its own filter: the default list's
    // conversations, one past its bound, do not count against it.
    stored_many(&listing, "org", "person", bound, true, |n| 10_000 + n);
    let archived = listing.service.list(owner(), true).await.unwrap();
    assert_eq!(archived.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(archived.complete);
    stored_many(&listing, "org", "person", 1, true, |_| 20_000);
    let archived = listing.service.list(owner(), true).await.unwrap();
    assert_eq!(archived.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(!archived.complete);
}

#[tokio::test]
async fn an_unreadable_row_makes_its_owners_list_incomplete_and_nobody_elses() {
    let listing = stored(ConversationLimits::default());
    let mine = id();
    stored_put(&listing, &mine, "org", "person", 1).await;
    stored_say(&listing, &mine, "mine", 2).await;
    let theirs = id();
    stored_put(&listing, &theirs, "org", "other", 1).await;
    stored_say(&listing, &theirs, "theirs", 2).await;
    assert!(listing.service.list(owner(), false).await.unwrap().complete);
    // Somebody else's record damaged: whose it is still reads, and it is not
    // the caller's, so the caller's list is whole. Its owner's is not.
    damage(
        &listing,
        "UPDATE conversations SET creation_action = '' WHERE id = ?1",
        &theirs,
    );
    let listed = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(ids(&listed.conversations), [mine.to_string()]);
    assert!(listed.complete);
    let other = listing
        .service
        .list(caller("org", "other"), false)
        .await
        .unwrap();
    assert!(other.conversations.is_empty());
    assert!(!other.complete);
    // The caller's own: left out, and the caller's list says so, in the list
    // its flag files it under.
    damage(
        &listing,
        "UPDATE conversations SET creation_action = '' WHERE id = ?1",
        &mine,
    );
    let listed = listing.service.list(owner(), false).await.unwrap();
    assert!(listed.conversations.is_empty());
    assert!(!listed.complete);
    assert!(listing.service.list(owner(), true).await.unwrap().complete);
    // And a stranger's list, which never met either, is whole.
    assert!(
        listing
            .service
            .list(caller("elsewhere", "someone"), false)
            .await
            .unwrap()
            .complete
    );
}

#[tokio::test]
async fn a_list_reads_only_the_callers_conversations_however_many_others_there_are() {
    let listing = stored(ConversationLimits::default());
    // Fifty thousand conversations of other people's, one of them damaged, and
    // three of the caller's.
    let others = stored_many(&listing, "org", "someone", 50_000, false, |n| n);
    damage(
        &listing,
        "UPDATE conversations SET creator_surface = '' WHERE id = ?1",
        &others[0],
    );
    let mine = stored_many(&listing, "org", "person", 3, false, |n| 100 + n);
    let listed = listing.service.list(owner(), false).await.unwrap();
    assert!(listed.complete);
    assert_eq!(
        ids(&listed.conversations),
        mine.iter()
            .rev()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    // What the list cost is the store's to show: its query reads only the
    // owner's rows (`the_list_query_reads_only_its_owners_rows_however_many_others_there_are`).
}

#[tokio::test]
async fn the_archive_filter_applies_before_the_bound() {
    let listing = stored(ConversationLimits::default());
    // A whole list of the caller's own archived conversations, every one of
    // them newer than the unarchived ones.
    let archived = stored_many(
        &listing,
        "org",
        "person",
        MAX_LISTED_CONVERSATIONS as u64 + 5,
        true,
        |n| 10_000 + n,
    );
    let unarchived = stored_many(&listing, "org", "person", 3, false, |n| n);
    // Archived ones take no place in the default list...
    let listed = listing.service.list(owner(), false).await.unwrap();
    assert!(listed.complete);
    assert_eq!(
        ids(&listed.conversations),
        unarchived
            .iter()
            .rev()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert!(listed.conversations.iter().all(|entry| !entry.archived));
    // ...and asked for, they are bounded like any list, newest first.
    let listed = listing.service.list(owner(), true).await.unwrap();
    assert!(!listed.complete);
    assert_eq!(listed.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert_eq!(
        ids(&listed.conversations),
        archived
            .iter()
            .rev()
            .take(MAX_LISTED_CONVERSATIONS)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert!(listed.conversations.iter().all(|entry| entry.archived));
}

#[tokio::test]
async fn a_list_the_store_cannot_answer_fails_rather_than_showing_nothing() {
    let listing = stored(ConversationLimits::default());
    let mine = id();
    stored_put(&listing, &mine, "org", "person", 1).await;
    stored_say(&listing, &mine, "mine", 2).await;
    Connection::open(&listing.path)
        .unwrap()
        .execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE summaries;")
        .unwrap();
    for archived in [false, true] {
        assert!(matches!(
            listing.service.list(owner(), archived).await,
            Err(ConversationError::Metadata)
        ));
    }
}

/// A listing that answers nothing and remembers how many rows it was asked for.
#[derive(Default)]
struct AskedLimit(std::sync::Mutex<Vec<usize>>);
impl ConversationListing for AskedLimit {
    fn list(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        limit: usize,
    ) -> crate::conversation::application::ConversationFuture<'_, ListedConversations> {
        self.0.lock().unwrap().push(limit);
        Box::pin(async { Ok(ListedConversations::default()) })
    }
}

#[tokio::test]
async fn a_list_asks_for_one_row_past_its_bound_and_no_more() {
    let asked = Arc::new(AskedLimit::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(
                Arc::new(ProviderFactory::default()),
            ))),
            storage: Arc::new(CountingStorage::default()),
            metadata: Arc::new(MemoryRepository::default()),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: asked.clone(),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    for archived in [false, true] {
        assert!(service.list(owner(), archived).await.unwrap().complete);
    }
    assert_eq!(
        *asked.0.lock().unwrap(),
        [MAX_LISTED_CONVERSATIONS + 1, MAX_LISTED_CONVERSATIONS + 1]
    );
}
