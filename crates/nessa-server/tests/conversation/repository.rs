//! Durable ownership must preserve the original caller and reject corrupt metadata.
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
