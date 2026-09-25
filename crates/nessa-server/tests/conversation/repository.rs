//! Durable ownership must preserve the original caller and reject corrupt metadata.
use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{
        ConversationCreationDisposition, ConversationError, ConversationRecords,
        ConversationRepository,
    },
    domain::{Conversation, ConversationDeletion, ConversationId, ProviderSessionLink},
    infrastructure::LocalConversationRepository,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_storage::{self as storage, OpenMode};
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::io::Write;
use uuid::Uuid;

#[tokio::test]
async fn ownership_is_create_once_and_corrupt_records_fail_closed() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-test-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let original = Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        "create-original".into(),
        123,
        AgentId::Claude,
    )
    .unwrap();
    let created = repository.create(original.clone()).await.unwrap();
    assert_eq!(created.conversation, original);
    assert_eq!(
        created.disposition,
        crate::conversation::application::ConversationCreationDisposition::Created
    );
    let impostor = Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("bob").unwrap(),
        "other".into(),
        "overwrite".into(),
        456,
        AgentId::Claude,
    )
    .unwrap();
    let existing = repository.create(impostor).await.unwrap();
    assert_eq!(existing.conversation, original);
    assert_eq!(
        existing.disposition,
        crate::conversation::application::ConversationCreationDisposition::Existing
    );
    drop(repository);
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    assert_eq!(repository.load(&id).await.unwrap(), Some(original));
    let path = root.join(format!("{id}.json"));
    std::fs::remove_file(&path).unwrap();
    let mut file = storage::open(&path, OpenMode::CreateNew).unwrap();
    file.write_all(br#"{"id":"invalid"}"#).unwrap();
    drop(file);
    assert!(matches!(
        repository.load(&id).await,
        Err(ConversationError::Metadata)
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn interrupted_creation_never_publishes_a_partial_record_or_poisons_its_id() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-test-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let owner = Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        "create-original".into(),
        123,
        AgentId::Claude,
    )
    .unwrap();

    // A creation interrupted while writing leaves its unfinished bytes under a
    // private temporary name; the conversation ID itself is still unused.
    let unfinished = root.join(".nessa-interrupted-write.tmp");
    storage::open(&unfinished, OpenMode::CreateNew)
        .unwrap()
        .write_all(br#"{"id":"#)
        .unwrap();
    assert!(!root.join(format!("{id}.json")).exists());
    assert_eq!(repository.load(&id).await.unwrap(), None);

    // Retrying the same identity completes, and reopening the directory
    // releases the unfinished temporary.
    let created = repository.create(owner.clone()).await.unwrap();
    assert_eq!(created.conversation, owner);
    assert_eq!(
        created.disposition,
        crate::conversation::application::ConversationCreationDisposition::Created
    );
    drop(repository);
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    assert!(!unfinished.exists());
    assert_eq!(repository.load(&id).await.unwrap(), Some(owner.clone()));

    // A publish interrupted after linking its destination but before releasing
    // its own name recovers when the next owner takes the directory.
    let leftover = root.join(".nessa-interrupted-publish.tmp");
    std::fs::hard_link(root.join(format!("{id}.json")), &leftover).unwrap();
    assert!(matches!(
        repository.load(&id).await,
        Err(ConversationError::Metadata)
    ));
    drop(repository);
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    assert!(!leftover.exists());
    assert_eq!(repository.load(&id).await.unwrap(), Some(owner));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_record_carries_the_agent_it_was_created_on() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-agent-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let record = Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        "create".into(),
        123,
        AgentId::Codex,
    )
    .unwrap();
    repository.create(record.clone()).await.unwrap();
    drop(repository);
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    assert_eq!(repository.load(&id).await.unwrap(), Some(record));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn a_record_that_does_not_name_its_agent_is_unreadable_rather_than_assumed() {
    // Records published while Claude was the only agent this server could start
    // name no agent at all. Reading them as Claude's would be a reader for data
    // written by an older build, which this repository forbids outright without
    // an explicit decision to support compatibility. So it is refused — and
    // refused as an agent this build cannot open, which is what happened: the
    // record parsed, storage is working, and asking again will say the same.
    //
    // Reporting it as unreadable metadata would reach the panel as
    // `conversation_storage_unavailable`, which tells the caller storage is
    // down and to retry something that can only be fixed by retrofitting the
    // record. `scripts/retrofit-conversation-agents.mjs` is what fixes it.
    let root = std::env::temp_dir().join(format!("nessa-conversation-legacy-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut file = storage::open(&root.join(format!("{id}.json")), OpenMode::CreateNew).unwrap();
    file.write_all(
        format!(
            r#"{{"id":"{id}","organization":"org","owner":"alice","creator_surface":"panel","creation_action":"create","creation_requested_at_ms":123}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    assert!(matches!(
        repository.load(&id).await,
        Err(ConversationError::AgentUnsupported)
    ));

    // And a record that is genuinely damaged is still unreadable metadata. The
    // two must not collapse into one answer: one is retried and one is not.
    let broken = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut file =
        storage::open(&root.join(format!("{broken}.json")), OpenMode::CreateNew).unwrap();
    file.write_all(br#"{"id":"truncated"#).unwrap();
    drop(file);
    assert!(matches!(
        repository.load(&broken).await,
        Err(ConversationError::Metadata)
    ));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn a_record_naming_an_agent_this_build_cannot_start_is_read_with_no_agent_and_can_be_deleted()
{
    let root = std::env::temp_dir().join(format!("nessa-conversation-unknown-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut file = storage::open(&root.join(format!("{id}.json")), OpenMode::CreateNew).unwrap();
    file.write_all(
        format!(
            r#"{{"id":"{id}","organization":"org","owner":"alice","creator_surface":"panel","creation_action":"create","creation_requested_at_ms":123,"agent":"gemini"}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    // Read, with no agent: never taken for another agent's conversation, and
    // never "storage is unavailable" — the record was read without trouble.
    // Opening it is refused where it would be opened, as `AgentUnsupported`.
    let loaded = repository.load(&id).await.unwrap().unwrap();
    assert_eq!(loaded.agent(), None);
    assert_eq!(loaded.owner(), &PrincipalId::new("alice").unwrap());
    // It is listed, and its owner can delete it: neither needs the agent.
    assert_eq!(repository.list().await.unwrap().conversations, [loaded]);
    let deleted = repository
        .record_deletion(
            &id,
            ConversationDeletion::new(
                OrganizationId::new("org").unwrap(),
                PrincipalId::new("alice").unwrap(),
                "panel".into(),
                "delete".into(),
                200,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(deleted.deletion().is_some());
    assert_eq!(repository.load(&id).await.unwrap().unwrap(), deleted);
    // It is not written again as some other agent's record either.
    assert!(matches!(
        repository.create(deleted.clone()).await,
        Ok(created) if created.conversation == deleted
    ));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn a_record_missing_more_than_its_agent_is_damaged_and_not_a_pre_agent_record() {
    // The case the classifier exists to get right, and the one nothing failed
    // on before: a file that parses as JSON, names no agent, and is not
    // otherwise a current record. It must not be waved through as "published
    // before there was a second agent", because it was not — something is wrong
    // with it, and the caller is owed the answer that says so.
    //
    // A truncated file cannot show this. It never parses as JSON at all, so it
    // is refused by the first line of the classifier and the question of what
    // the remaining fields look like is never asked.
    let root = std::env::temp_dir().join(format!("nessa-conversation-damaged-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();

    // Well-formed, no `agent`, and no `owner` either. Supplying the missing
    // agent still leaves a record this build would not have written.
    let missing = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut file =
        storage::open(&root.join(format!("{missing}.json")), OpenMode::CreateNew).unwrap();
    file.write_all(
        format!(
            r#"{{"id":"{missing}","organization":"org","creator_surface":"panel","creation_action":"create","creation_requested_at_ms":123}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    assert!(matches!(
        repository.load(&missing).await,
        Err(ConversationError::Metadata)
    ));

    // Well-formed, no `agent`, and a field this build does not know. The record
    // type refuses unknown keys, so this is not a pre-agent record either.
    let extra = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut file = storage::open(&root.join(format!("{extra}.json")), OpenMode::CreateNew).unwrap();
    file.write_all(
        format!(
            r#"{{"id":"{extra}","organization":"org","owner":"alice","creator_surface":"panel","creation_action":"create","creation_requested_at_ms":123,"model":"opus"}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    assert!(matches!(
        repository.load(&extra).await,
        Err(ConversationError::Metadata)
    ));

    // And one that names a different conversation than the file it is in. The
    // agreement between the two is checked on the ordinary path, which a record
    // naming no agent never reaches; without the same check in the classifier
    // this reads as an agent this build cannot open.
    let mismatched = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let other = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let mut file = storage::open(
        &root.join(format!("{mismatched}.json")),
        OpenMode::CreateNew,
    )
    .unwrap();
    file.write_all(
        format!(
            r#"{{"id":"{other}","organization":"org","owner":"alice","creator_surface":"panel","creation_action":"create","creation_requested_at_ms":123}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    drop(file);
    assert!(matches!(
        repository.load(&mismatched).await,
        Err(ConversationError::Metadata)
    ));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn listing_returns_every_readable_record_and_leaves_out_damaged_ones() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-test-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    assert_eq!(
        repository.list().await.unwrap(),
        ConversationRecords::default()
    );
    let mut stored = Vec::new();
    for (owner, at) in [("alice", 1), ("bob", 2)] {
        let conversation = Conversation::new(
            ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new(owner).unwrap(),
            "panel".into(),
            "create".into(),
            at,
            AgentId::Claude,
        )
        .unwrap();
        repository.create(conversation.clone()).await.unwrap();
        stored.push(conversation);
    }
    // A damaged record, a record under a name that is not a conversation, and
    // a publish still in progress: none is a conversation to list, and none
    // stops the others being listed.
    let damaged = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    for (name, bytes) in [
        (format!("{damaged}.json"), &br#"{"id":"#[..]),
        ("notes.txt".to_owned(), &b"not a record"[..]),
        (format!(".nessa-{}.tmp", "0".repeat(32)), &br#"{"id":"#[..]),
    ] {
        storage::open(&root.join(name), OpenMode::CreateNew)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }
    let listed = repository.list().await.unwrap();
    // Only the damaged record is counted as one that could not be read: the
    // other two are not conversation records at all.
    assert_eq!(listed.unreadable, 1);
    let mut listed = listed.conversations;
    listed.sort_by_key(Conversation::creation_requested_at_ms);
    assert_eq!(listed, stored);
    // The damaged record is still refused where it is opened.
    assert!(matches!(
        repository.load(&damaged).await,
        Err(ConversationError::Metadata)
    ));

    // Not being able to enumerate at all is the one failure.
    std::fs::remove_dir_all(&root).unwrap();
    assert!(matches!(
        repository.list().await,
        Err(ConversationError::Metadata)
    ));
}

fn owned(id: &ConversationId) -> Conversation {
    Conversation::new(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        "create".into(),
        1,
        AgentId::Claude,
    )
    .unwrap()
}
fn deletion(request: &str) -> ConversationDeletion {
    ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("alice").unwrap(),
        "panel".into(),
        request.into(),
        50,
    )
    .unwrap()
}

#[tokio::test]
async fn a_tombstone_whose_record_was_moved_aside_is_counted_not_listed() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-test-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let alive = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    repository.create(owned(&id)).await.unwrap();
    repository.create(owned(&alive)).await.unwrap();
    repository
        .record_deletion(&id, deletion("delete-1"))
        .await
        .unwrap();
    // A tombstone with its record is neither.
    let listed = repository.list().await.unwrap();
    assert_eq!(listed.conversations.len(), 2);
    assert_eq!((listed.unreadable, listed.orphaned_tombstones), (0, 0));
    // Its record moved out of the directory, as for a damaged one.
    std::fs::rename(
        root.join(format!("{id}.json")),
        root.with_extension("aside"),
    )
    .unwrap();
    let listed = repository.list().await.unwrap();
    assert_eq!(
        listed
            .conversations
            .iter()
            .map(Conversation::id)
            .collect::<Vec<_>>(),
        [&alive]
    );
    // Not a record that could not be read — it is a deleted conversation,
    // missing from no list — but a deletion that cannot be finished.
    assert_eq!((listed.unreadable, listed.orphaned_tombstones), (0, 1));
    assert!(matches!(
        repository.load(&id).await,
        Err(ConversationError::Metadata)
    ));
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_file(root.with_extension("aside")).ok();
}

#[tokio::test]
async fn a_tombstone_outlives_reopening_and_its_identity_is_never_created_again() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-test-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let alive = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    repository.create(owned(&id)).await.unwrap();
    repository.create(owned(&alive)).await.unwrap();
    // Nothing to delete without a record.
    let absent = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    assert!(matches!(
        repository
            .record_deletion(&absent, deletion("delete-0"))
            .await,
        Err(ConversationError::NotFound)
    ));

    let first = repository
        .record_deletion(&id, deletion("delete-1"))
        .await
        .unwrap();
    assert_eq!(first.deletion(), Some(&deletion("delete-1")));
    // A later request's delete does not replace the decision; the same
    // decision fills in what it read of the history, once.
    let later = repository
        .record_deletion(&id, deletion("delete-2"))
        .await
        .unwrap();
    assert_eq!(later.deletion(), Some(&deletion("delete-1")));
    let session = ExecutionSessionId::new("provider-session").unwrap();
    repository
        .record_deletion(
            &id,
            deletion("delete-1").after_reading(Some(session.clone())),
        )
        .await
        .unwrap();

    drop(repository);
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let loaded = repository.load(&id).await.unwrap().unwrap();
    let tombstone = loaded.deletion().unwrap();
    assert_eq!(tombstone.request(), "delete-1");
    assert_eq!(
        tombstone.provider_session(),
        &ProviderSessionLink::Recorded(session)
    );
    // Creating the identity again finds it, deleted, rather than making it.
    let created = repository.create(owned(&id)).await.unwrap();
    assert_eq!(
        created.disposition,
        ConversationCreationDisposition::Existing
    );
    assert!(created.conversation.deletion().is_some());
    // Listed with its tombstone, beside the conversation that is not deleted;
    // the tombstones' directory is not itself a conversation.
    let mut listed = repository.list().await.unwrap().conversations;
    listed.sort_by_key(|conversation| conversation.deletion().is_some());
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id(), &alive);
    assert!(listed[0].deletion().is_none());
    assert_eq!(listed[1].id(), &id);
    assert!(listed[1].deletion().is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_damaged_or_orphaned_tombstone_is_refused_and_never_read_as_absent() {
    let root = std::env::temp_dir().join(format!("nessa-conversation-test-{}", Uuid::new_v4()));
    let repository = LocalConversationRepository::new(root.clone()).unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let other = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    repository.create(owned(&id)).await.unwrap();
    let path = root.join("deleted").join(format!("{id}.json"));
    let valid = |id: &ConversationId, session: &str| {
        format!(
            r#"{{"id":"{id}","organization":"org","initiator":"alice","surface":"panel","request":"delete","requested_at_ms":1,"provider_session":{session},"provider_erasure":null,"erased":false}}"#
        )
    };
    // One that could not stand beside its record is not written either.
    let mallory = ConversationDeletion::new(
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("mallory").unwrap(),
        "panel".into(),
        "delete".into(),
        50,
    )
    .unwrap();
    assert!(matches!(
        repository.record_deletion(&id, mallory).await,
        Err(ConversationError::Metadata)
    ));
    assert!(!path.exists());
    for bytes in [
        "{".to_owned(),
        // Another conversation's tombstone under this one's name.
        valid(&other, r#"{"state":"unread"}"#),
        // A provider session that is not one.
        valid(&id, r#"{"state":"recorded","id":""}"#),
        // Read and not read at once.
        valid(&id, r#"{"state":"absent","id":"x"}"#),
        // A request that could not have been written down.
        valid(&id, r#"{"state":"unread"}"#).replace("\"delete\"", "\"line\\nbreak\""),
        // A field this build does not know.
        valid(&id, r#"{"state":"unread"}"#).replacen(
            r#""requested_at_ms":1"#,
            r#""requested_at_ms":1,"extra":1"#,
            1,
        ),
        // Deleted by somebody who is not the owner, or not in its
        // organization, or before the conversation existed.
        valid(&id, r#"{"state":"unread"}"#).replace(r#""alice""#, r#""mallory""#),
        valid(&id, r#"{"state":"unread"}"#).replace(r#""org""#, r#""elsewhere""#),
        valid(&id, r#"{"state":"unread"}"#)
            .replace(r#""requested_at_ms":1"#, r#""requested_at_ms":0"#),
        // Progress no deletion reaches: the agent's record settled before the
        // history was read, and erased before the agent's record was settled.
        valid(&id, r#"{"state":"unread"}"#).replace(
            r#""provider_erasure":null"#,
            r#""provider_erasure":"deleted""#,
        ),
        valid(&id, r#"{"state":"recorded","id":"s"}"#)
            .replace(r#""erased":false"#, r#""erased":true"#),
        // Read as naming none, and left unsettled, which reading never does.
        valid(&id, r#"{"state":"absent"}"#),
    ] {
        let _ = std::fs::remove_file(&path);
        storage::open(&path, OpenMode::CreateNew)
            .unwrap()
            .write_all(bytes.as_bytes())
            .unwrap();
        assert!(
            matches!(repository.load(&id).await, Err(ConversationError::Metadata)),
            "{bytes}"
        );
        // Refused, never repaired: the file is as it was.
        assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
        // Refused for creation too: never taken for a free identity.
        assert!(
            matches!(
                repository.create(owned(&id)).await,
                Err(ConversationError::Metadata)
            ),
            "{bytes}"
        );
    }
    // A tombstone whose record is missing is damage, not an identity that can
    // be created again.
    std::fs::remove_file(&path).unwrap();
    let orphan = root.join("deleted").join(format!("{other}.json"));
    storage::open(&orphan, OpenMode::CreateNew)
        .unwrap()
        .write_all(valid(&other, r#"{"state":"unread"}"#).as_bytes())
        .unwrap();
    assert!(matches!(
        repository.load(&other).await,
        Err(ConversationError::Metadata)
    ));
    assert!(matches!(
        repository.create(owned(&other)).await,
        Err(ConversationError::Metadata)
    ));
    assert!(!root.join(format!("{other}.json")).exists());
    std::fs::remove_dir_all(root).unwrap();
}
