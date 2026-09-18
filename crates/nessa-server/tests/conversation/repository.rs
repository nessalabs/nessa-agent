//! Durable ownership must preserve the original caller and reject corrupt metadata.
use crate::agents::domain::AgentId;
use crate::conversation::{
    application::{ConversationError, ConversationRepository},
    domain::{Conversation, ConversationId},
    infrastructure::LocalConversationRepository,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_storage::{self as storage, OpenMode};
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
    // an explicit decision to support compatibility. So it is refused, and the
    // error names the field rather than guessing which agent wrote it.
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
        Err(ConversationError::Metadata)
    ));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn a_record_naming_an_agent_this_build_cannot_start_is_not_opened_on_another() {
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
    // Its own failure, and never "storage is unavailable": the record was read
    // without trouble and a retry cannot change the answer, so a caller told
    // storage was down would go on asking a question this build cannot answer.
    let failure = repository.load(&id).await;
    assert!(matches!(failure, Err(ConversationError::AgentUnsupported)));
    assert!(!matches!(failure, Err(ConversationError::Metadata)));
    std::fs::remove_dir_all(root).ok();
}
