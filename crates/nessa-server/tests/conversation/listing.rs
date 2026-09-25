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
    conversation_test_support::{
        only, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryRepository, MemorySummaries,
        Provider, ProviderFactory, RecordingFileLinkAudit, TestClock, DELETION_BUDGETS,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
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
    storage: Arc<CountingStorage>,
}
fn listing(limits: ConversationLimits) -> Listing {
    let provider = Arc::new(ProviderFactory::default());
    let repository = Arc::new(MemoryRepository::default());
    let summaries = Arc::new(MemorySummaries::default());
    let storage = Arc::new(CountingStorage::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: summaries.clone(),
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
        storage,
    }
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
    let listing = listing(ConversationLimits {
        max_conversations: 1,
        ..ConversationLimits::default()
    });
    let mine = id();
    let another_principal = id();
    let another_organization = id();
    put(&listing, &mine, "org", "person", 10);
    summarize(&listing, &mine, "mine", 15);
    put(&listing, &another_principal, "org", "other", 20);
    summarize(&listing, &another_principal, "not yours", 40);
    put(&listing, &another_organization, "elsewhere", "person", 30);
    summarize(&listing, &another_organization, "not yours either", 50);

    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(
        listed,
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
    assert_eq!(
        ids(&listing
            .service
            .list(caller("org", "other"), false)
            .await
            .unwrap()
            .conversations),
        [another_principal.to_string()]
    );
    assert_eq!(
        ids(&listing
            .service
            .list(caller("elsewhere", "person"), false)
            .await
            .unwrap()
            .conversations),
        [another_organization.to_string()]
    );
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
    summarize(&listing, &created, "live", 60);
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
    let listing = listing(ConversationLimits::default());
    // More than a whole list of somebody else's conversations, every one of
    // them newer than anything the caller has.
    for n in 0..MAX_LISTED_CONVERSATIONS as u64 + 100 {
        let foreign = id();
        put(&listing, &foreign, "org", "other", 1_000 + n);
        summarize(&listing, &foreign, "newer", 10_000 + n);
    }
    let mine: Vec<_> = (0..3).map(|_| id()).collect();
    for (n, conversation) in mine.iter().enumerate() {
        put(&listing, conversation, "org", "person", n as u64);
        summarize(&listing, conversation, "mine", n as u64);
    }
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(
        ids(&listed),
        mine.iter()
            .rev()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );

    // Past the bound, the oldest of the caller's own are the ones left out.
    let mut own: Vec<_> = mine.clone();
    for n in 3..MAX_LISTED_CONVERSATIONS as u64 + 10 {
        let conversation = id();
        put(&listing, &conversation, "org", "person", n);
        summarize(&listing, &conversation, "mine", n);
        own.push(conversation);
    }
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
async fn newest_summary_first_then_identity_and_an_unreadable_summary_lists_bare() {
    let listing = listing(ConversationLimits::default());
    let [older, tied_low, tied_high] = {
        let mut three = [id(), id(), id()];
        three[1..].sort_by_key(ToString::to_string);
        three
    };
    put(&listing, &older, "org", "person", 50);
    summarize(&listing, &older, "older", 50);
    put(&listing, &tied_low, "org", "person", 1);
    put(&listing, &tied_high, "org", "person", 2);
    summarize(&listing, &tied_low, "low", 100);
    summarize(&listing, &tied_high, "high", 100);
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(
        ids(&listed),
        [
            tied_low.to_string(),
            tied_high.to_string(),
            older.to_string()
        ]
    );
    assert_eq!(listed[0].title.as_deref(), Some("low"));
    assert_eq!(listed[0].preview.as_deref(), Some("low"));
    assert_eq!(listed[0].created_at_ms, 1);
    assert_eq!(listed[0].updated_at_ms, 100);

    // A summary store that cannot be read costs the rows their summaries,
    // not the list.
    // And one somebody archived.
    let archived = id();
    put(&listing, &archived, "org", "person", 3);
    archive(&listing, &archived, 3);
    assert!(!ids(&listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations)
    .contains(&archived.to_string()));
    listing.summaries.load_fails.store(true, Ordering::SeqCst);
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(ids(&listed)[0], older.to_string());
    // The archived one too: that it was archived cannot be read.
    assert!(ids(&listed).contains(&archived.to_string()));
    assert!(listed.iter().all(|entry| entry.title.is_none()
        && entry.preview.is_none()
        && entry.updated_at_ms == entry.created_at_ms
        && !entry.archived));
    assert_eq!(listed.len(), 4);
    // In the default list only: whether they were archived cannot be read.
    assert!(listing
        .service
        .list(owner(), true)
        .await
        .unwrap()
        .conversations
        .is_empty());
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

fn archive(listing: &Listing, id: &ConversationId, at: u64) {
    let mut summaries = listing.summaries.summaries.lock().unwrap();
    let said = summaries
        .get(id)
        .cloned()
        .unwrap_or_else(|| ConversationSummary::after_message(None, "said", None, at));
    let archived = said.after_archiving(true);
    summaries.insert(id.clone(), archived);
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
    let listing = listing(ConversationLimits::default());
    let bound = MAX_LISTED_CONVERSATIONS as u64;
    // Another principal's conversations, and the caller's own that nothing
    // was said in, are not the caller's to list and never make it incomplete.
    for n in 0..bound + 50 {
        let foreign = id();
        put(&listing, &foreign, "org", "other", n);
        summarize(&listing, &foreign, "theirs", n);
    }
    for n in 0..10 {
        put(&listing, &id(), "org", "person", n);
    }
    let mut own = 0;
    let mut add = |archived: bool| {
        let conversation = id();
        own += 1;
        put(&listing, &conversation, "org", "person", 1_000 + own);
        if archived {
            archive(&listing, &conversation, 1_000 + own);
        } else {
            summarize(&listing, &conversation, "mine", 1_000 + own);
        }
    };
    for _ in 0..bound - 1 {
        add(false);
    }
    let under = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(under.conversations.len(), MAX_LISTED_CONVERSATIONS - 1);
    assert!(under.complete);
    // Exactly at the bound, with nothing behind it: all of them.
    add(false);
    let at = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(at.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(at.complete);
    // One more behind the bound: the list is cut, and says so.
    add(false);
    let past = listing.service.list(owner(), false).await.unwrap();
    assert_eq!(past.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(!past.complete);

    // The archived list is judged by its own filter: the default list's
    // conversations, one past its bound, do not count against it.
    for _ in 0..bound {
        add(true);
    }
    let archived = listing.service.list(owner(), true).await.unwrap();
    assert_eq!(archived.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(archived.complete);
    add(true);
    let archived = listing.service.list(owner(), true).await.unwrap();
    assert_eq!(archived.conversations.len(), MAX_LISTED_CONVERSATIONS);
    assert!(!archived.complete);
}

#[tokio::test]
async fn an_unreadable_record_makes_every_list_incomplete() {
    let root = std::env::temp_dir().join(format!("nessa-listing-test-{}", uuid::Uuid::new_v4()));
    let repository = Arc::new(
        crate::conversation::infrastructure::LocalConversationRepository::new(root.clone())
            .unwrap(),
    );
    let summaries = Arc::new(MemorySummaries::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(
                Arc::new(ProviderFactory::default()),
            ))),
            storage: Arc::new(CountingStorage::default()),
            metadata: repository.clone(),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: summaries.clone(),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let conversation = id();
    crate::conversation::application::ConversationRepository::create(
        repository.as_ref(),
        Conversation::new(
            conversation.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "panel".into(),
            "create".into(),
            1,
            AgentId::Claude,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    summaries.summaries.lock().unwrap().insert(
        conversation.clone(),
        ConversationSummary::after_message(None, "mine", None, 2),
    );
    let listed = service.list(owner(), false).await.unwrap();
    assert_eq!(listed.conversations.len(), 1);
    assert!(listed.complete);
    // A deleted conversation whose record was moved aside leaves a tombstone
    // alone: missing from no list, so every list is still complete, but a
    // deletion the start's finish can neither read nor finish, and counts.
    let orphan = id();
    let repository_port: &dyn crate::conversation::application::ConversationRepository =
        repository.as_ref();
    repository_port
        .create(
            Conversation::new(
                orphan.clone(),
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "create".into(),
                1,
                AgentId::Claude,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    repository_port
        .record_deletion(
            &orphan,
            crate::conversation::domain::ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("person").unwrap(),
                "panel".into(),
                "delete".into(),
                3,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    std::fs::remove_file(root.join(format!("{orphan}.json"))).unwrap();
    assert!(service.list(owner(), false).await.unwrap().complete);
    let left = service.finish_deletions().await.unwrap();
    assert_eq!((left.unreadable, left.orphaned_tombstones), (0, 1));
    std::fs::remove_file(root.join("deleted").join(format!("{orphan}.json"))).unwrap();
    // Its ownership record damaged: whose it is cannot be read, so it is
    // left out of every list, and no list claims to name everything.
    std::fs::write(root.join(format!("{conversation}.json")), br#"{"id":"#).unwrap();
    for archived in [false, true] {
        let listed = service.list(owner(), archived).await.unwrap();
        assert!(listed.conversations.is_empty(), "{archived}");
        assert!(!listed.complete, "{archived}");
    }
    // Every caller's list, not only its owner's: whose it is cannot be read.
    let stranger = service
        .list(caller("elsewhere", "someone"), false)
        .await
        .unwrap();
    assert!(!stranger.complete);
    // And a start's finish counts it, since it may be a deletion it cannot see.
    let left = service.finish_deletions().await.unwrap();
    assert!(left.unfinished.is_empty());
    assert_eq!((left.unreadable, left.orphaned_tombstones), (1, 0));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn the_archive_filter_applies_before_the_bound() {
    let listing = listing(ConversationLimits::default());
    // A whole list of the caller's own archived conversations, every one of
    // them newer than the unarchived ones.
    let mut archived = Vec::new();
    for n in 0..MAX_LISTED_CONVERSATIONS as u64 + 5 {
        let conversation = id();
        put(&listing, &conversation, "org", "person", 10_000 + n);
        archive(&listing, &conversation, 10_000 + n);
        archived.push(conversation);
    }
    let unarchived: Vec<_> = (0..3).map(|_| id()).collect();
    for (n, conversation) in unarchived.iter().enumerate() {
        put(&listing, conversation, "org", "person", n as u64);
        summarize(&listing, conversation, "unarchived", n as u64);
    }
    // Archived ones take no place in the default list...
    let listed = listing
        .service
        .list(owner(), false)
        .await
        .unwrap()
        .conversations;
    assert_eq!(
        ids(&listed),
        unarchived
            .iter()
            .rev()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert!(listed.iter().all(|entry| !entry.archived));
    // ...and asked for, they are bounded like any list, newest first.
    let listed = listing
        .service
        .list(owner(), true)
        .await
        .unwrap()
        .conversations;
    assert_eq!(listed.len(), MAX_LISTED_CONVERSATIONS);
    assert_eq!(
        ids(&listed),
        archived
            .iter()
            .rev()
            .take(MAX_LISTED_CONVERSATIONS)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
    assert!(listed.iter().all(|entry| entry.archived));
}
