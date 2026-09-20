//! The adapters other contexts reach attachments through, each against the
//! port it implements.
use super::*;
use crate::{
    attachments::application::{AttachmentLimits, AttachmentStore, ConversationOwnership},
    attachments_test_support::{
        conversation, digest_of, organization, principal, Fixture, StubNormalizer, CONVERSATION,
        OTHER_CONVERSATION,
    },
    conversation::{
        application::{
            AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationError,
            ConversationRepository,
        },
        domain::Conversation,
    },
    conversation_test_support::MemoryRepository,
};
use nessa_sdk::{
    application::agent_execution::providers::{UserImageError, UserImageSource},
    domain::{agent_execution::prompts::ImageReference, common::value_objects::ImageMediaType},
};
use std::sync::{atomic::Ordering, Arc};

fn image(bytes: &[u8], media_type: ImageMediaType) -> ImageReference {
    ImageReference::new(digest_of(bytes), media_type, bytes.len() as u64).unwrap()
}
fn release(conversation_id: &str) -> AttachmentRelease {
    AttachmentRelease {
        organization_id: organization("org"),
        conversation_id: conversation(conversation_id),
        cause: AttachmentReleaseCause::ConversationClosed,
        initiator_principal_id: principal("owner"),
        initiator_surface_id: "panel".into(),
        correlation_id: "close-1".into(),
    }
}

#[tokio::test]
async fn a_conversation_holds_the_normalized_image_and_only_in_the_shape_it_was_kept() {
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(b"kept", "image/jpeg"),
    );
    fixture.upload(CONVERSATION, b"sent", "image/png").await;
    let holds = ConversationHolds::new(fixture.service.clone());
    let asks = |conversation_id: &'static str, image: ImageReference| {
        let holds = &holds;
        async move {
            holds
                .holds(&organization("org"), &conversation(conversation_id), &image)
                .await
                .unwrap()
        }
    };
    assert!(asks(CONVERSATION, image(b"kept", ImageMediaType::Jpeg)).await);
    // The right bytes under the wrong type, the uploaded file, another
    // conversation: each is a reference this conversation cannot use.
    assert!(!asks(CONVERSATION, image(b"kept", ImageMediaType::Png)).await);
    assert!(!asks(CONVERSATION, image(b"sent", ImageMediaType::Png)).await);
    assert!(!asks(OTHER_CONVERSATION, image(b"kept", ImageMediaType::Jpeg)).await);

    fixture.store.unavailable.store(true, Ordering::SeqCst);
    assert!(matches!(
        holds
            .holds(
                &organization("org"),
                &conversation(CONVERSATION),
                &image(b"kept", ImageMediaType::Jpeg)
            )
            .await,
        Err(ConversationError::Unavailable)
    ));
}

#[tokio::test]
async fn a_close_releases_through_the_service_and_reports_what_did_not_complete() {
    // Released and recorded.
    let fixture = Fixture::new(AttachmentLimits::default());
    fixture.upload(CONVERSATION, b"first", "text/plain").await;
    let holds = ConversationHolds::new(fixture.service.clone());
    assert!(holds.release(release(CONVERSATION)).await.is_ok());
    assert!(fixture.store.held().is_empty());

    // Released, but the evidence was lost: an audit failure, and still released.
    fixture.upload(CONVERSATION, b"second", "text/plain").await;
    fixture.audit.refusing.store(true, Ordering::SeqCst);
    assert!(matches!(
        holds.release(release(CONVERSATION)).await,
        Err(ConversationError::Audit)
    ));
    assert!(fixture.store.held().is_empty());

    // A file still in place is the failure a retry can act on, so it is the
    // one named, with or without lost evidence beside it.
    for refusing in [false, true] {
        fixture.audit.refusing.store(false, Ordering::SeqCst);
        let stuck = format!("stuck {refusing}").into_bytes();
        fixture.upload(CONVERSATION, &stuck, "text/plain").await;
        fixture.upload(CONVERSATION, b"free", "text/plain").await;
        fixture.store.stick(digest_of(&stuck));
        fixture.audit.refusing.store(refusing, Ordering::SeqCst);
        assert!(matches!(
            holds.release(release(CONVERSATION)).await,
            Err(ConversationError::Unavailable)
        ));
        // Everything that could go went; only what is stuck is still held.
        assert!(fixture
            .store
            .held()
            .iter()
            .all(|hold| hold.stored().size() != 4));
        assert!(fixture
            .store
            .held()
            .iter()
            .any(|hold| hold.stored().digest() == digest_of(&stuck)));
    }
}

#[tokio::test]
async fn ownership_is_the_conversation_contexts_own_rule_read_without_opening_anything() {
    let repository = Arc::new(MemoryRepository::default());
    repository
        .create(
            Conversation::new(
                conversation(CONVERSATION),
                organization("org"),
                principal("owner"),
                "panel".into(),
                "create".into(),
                1,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let ownership = RepositoryOwnership::new(repository);
    let owns = |organization_id: &'static str, principal_id: &'static str, id: &'static str| {
        let ownership = &ownership;
        async move {
            ownership
                .owns(
                    &organization(organization_id),
                    &principal(principal_id),
                    &conversation(id),
                )
                .await
        }
    };
    assert_eq!(owns("org", "owner", CONVERSATION).await, Ok(true));
    assert_eq!(owns("org", "other", CONVERSATION).await, Ok(false));
    assert_eq!(owns("other", "owner", CONVERSATION).await, Ok(false));
    assert_eq!(owns("org", "owner", OTHER_CONVERSATION).await, Ok(false));
}

#[tokio::test]
async fn image_bytes_are_missing_unavailable_or_the_bytes_and_never_more_than_asked() {
    let fixture = Fixture::with_normalizer(
        AttachmentLimits::default(),
        StubNormalizer::producing(b"kept image", "image/png"),
    );
    fixture.upload(CONVERSATION, b"sent", "image/png").await;
    let store: Arc<dyn AttachmentStore> = fixture.store.clone();
    let images = StoredUserImages::new(store);

    assert_eq!(
        images
            .read(image(b"kept image", ImageMediaType::Png))
            .await
            .unwrap(),
        b"kept image"
    );
    // By content alone: the uploaded bytes were never stored.
    assert_eq!(
        images.read(image(b"sent", ImageMediaType::Png)).await,
        Err(UserImageError::Missing)
    );
    // A reference that claims fewer bytes gets one more than it claimed and no
    // more, which is all the adapter needs to refuse it. This never decides
    // `Mismatch` itself.
    let short = ImageReference::new(digest_of(b"kept image"), ImageMediaType::Png, 4).unwrap();
    assert_eq!(images.read(short).await.unwrap(), b"kept ");

    fixture.store.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        images.read(image(b"kept image", ImageMediaType::Png)).await,
        Err(UserImageError::Unavailable)
    );
}
