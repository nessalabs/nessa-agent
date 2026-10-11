//! The adapters other contexts reach attachments through, each against the
//! port it implements.
use super::*;
use crate::{
    attachments::application::{
        AttachmentLimits, AttachmentStore, ConversationOwnership, Ownership,
    },
    attachments_test_support::{
        conversation, digest_of, organization, principal, Fixture, StubNormalizer, CONVERSATION,
        OTHER_CONVERSATION,
    },
    conversation::{
        application::{
            AttachmentRelease, AttachmentReleaseCause, ConversationAttachments, ConversationError,
            ConversationRepository,
        },
        domain::{Conversation, ConversationDeletion},
    },
    conversation_test_support::MemoryRepository,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
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
    // One hold released and its bytes removed: two records, neither delivered.
    assert!(matches!(
        holds.release(release(CONVERSATION)).await,
        Err(ConversationError::AttachmentCleanup {
            storage_failures: 0,
            audit_failures: 2
        })
    ));
    assert!(fixture.store.held().is_empty());

    // A file still in place and evidence that was lost are different failures.
    // Both cross the boundary; neither stands in for the other.
    for refusing in [false, true] {
        fixture.audit.refusing.store(false, Ordering::SeqCst);
        let stuck = format!("stuck {refusing}").into_bytes();
        fixture.upload(CONVERSATION, &stuck, "text/plain").await;
        fixture.upload(CONVERSATION, b"free", "text/plain").await;
        fixture.store.stick(digest_of(&stuck));
        fixture.audit.refusing.store(refusing, Ordering::SeqCst);
        // The first round's stuck hold is still stuck in the second.
        let (expected_storage_failures, expected_audit_failures) =
            if refusing { (2, 2) } else { (1, 0) };
        assert!(matches!(
            holds.release(release(CONVERSATION)).await,
            Err(ConversationError::AttachmentCleanup {
                storage_failures,
                audit_failures
            }) if storage_failures == expected_storage_failures
                && audit_failures == expected_audit_failures
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
                AgentId::Claude,
                ConversationModelId::new("test-model").unwrap(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let ownership = RepositoryOwnership::new(repository.clone());
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
    assert_eq!(
        owns("org", "owner", CONVERSATION).await,
        Ok(Ownership::Owned)
    );
    assert_eq!(
        owns("org", "other", CONVERSATION).await,
        Ok(Ownership::NotFound)
    );
    assert_eq!(
        owns("other", "owner", CONVERSATION).await,
        Ok(Ownership::NotFound)
    );
    assert_eq!(
        owns("org", "owner", OTHER_CONVERSATION).await,
        Ok(Ownership::NotFound)
    );
    // Deleted: its owner is told so, and anybody else is told nothing.
    repository
        .record_deletion(
            &conversation(CONVERSATION),
            ConversationDeletion::new(
                organization("org"),
                principal("owner"),
                "panel".into(),
                "delete".into(),
                2,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        owns("org", "owner", CONVERSATION).await,
        Ok(Ownership::Deleted)
    );
    assert_eq!(
        owns("org", "other", CONVERSATION).await,
        Ok(Ownership::NotFound)
    );
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

#[tokio::test]
async fn a_release_in_nobodys_name_is_refused_as_invalid_and_lets_go_of_nothing() {
    let fixture = Fixture::new(AttachmentLimits::default());
    fixture.upload(CONVERSATION, b"first", "text/plain").await;
    let holds = ConversationHolds::new(fixture.service.clone());
    let mut nobody = release(CONVERSATION);
    nobody.initiator_surface_id = " ".into();
    assert!(matches!(
        holds.release(nobody).await,
        Err(ConversationError::InvalidInput)
    ));
    assert_eq!(fixture.store.held().len(), 1);
}

/// A host's published bytes: each chunk in turn, then how the read ended.
struct HostBytes {
    chunks: std::vec::IntoIter<Vec<u8>>,
    end: Option<crate::conversation::application::ArtifactReadFailure>,
    read: Arc<std::sync::atomic::AtomicBool>,
}
impl crate::conversation::application::ArtifactBytes for HostBytes {
    fn next(&mut self) -> crate::conversation::application::ArtifactChunk<'_> {
        self.read.store(true, Ordering::SeqCst);
        let next = match self.chunks.next() {
            Some(chunk) => Ok(Some(chunk)),
            None => self.end.map_or(Ok(None), Err),
        };
        Box::pin(async move { next })
    }
}

fn offer(
    file: nessa_protocol::lease::StagedArtifact,
    chunks: &[&[u8]],
    end: Option<crate::conversation::application::ArtifactReadFailure>,
) -> (
    crate::conversation::application::PublishedArtifact,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let read = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let published = crate::conversation::application::PublishedArtifact {
        organization_id: organization("org"),
        conversation_id: conversation(CONVERSATION),
        lease: "lease-1".into(),
        requested_by: nessa_sdk::application::agent_execution::permissions::ActionContext::new(
            "owner",
            "phone",
            "request-1",
        )
        .unwrap(),
        file,
        bytes: Box::new(HostBytes {
            chunks: chunks
                .iter()
                .map(|chunk| chunk.to_vec())
                .collect::<Vec<_>>()
                .into_iter(),
            end,
            read: read.clone(),
        }),
        record: Box::new(|| Box::pin(async { true })),
    };
    (published, read)
}

/// What the host says it staged: `bytes` under the name it published.
fn staged(bytes: &[u8]) -> nessa_protocol::lease::StagedArtifact {
    let digest = digest_of(bytes).to_string();
    nessa_protocol::lease::StagedArtifact {
        name: "report.pdf".into(),
        media_type: "application/pdf".into(),
        size: bytes.len() as u64,
        digest: digest.strip_prefix("sha256:").unwrap_or(&digest).into(),
        path: "/outbox/0".into(),
    }
}

/// A published file is kept as its conversation's, once; what it is told
/// back says how a read failed, that bytes were not the file published, or
/// that what the host described cannot be kept at all.
#[tokio::test]
async fn a_published_file_is_kept_and_a_failed_read_says_why() {
    use crate::conversation::application::{ArtifactKept, ArtifactReadFailure};
    use nessa_protocol::lease::CollectionRefusal;
    let fixture = Fixture::new(AttachmentLimits::default());
    let holds = ConversationHolds::new(fixture.service.clone());
    let file: &[u8] = b"twenty bytes of file";
    for (end, refusal) in [
        (
            ArtifactReadFailure::LeaseEnded,
            CollectionRefusal::LeaseEnded,
        ),
        (ArtifactReadFailure::Changed, CollectionRefusal::Mismatch),
        (
            ArtifactReadFailure::Unavailable,
            CollectionRefusal::ChannelUnavailable,
        ),
    ] {
        let (published, _) = offer(staged(file), &[&file[..10]], Some(end));
        assert_eq!(
            holds.keep_published(published).await,
            Err(refusal),
            "{end:?}"
        );
    }
    let (published, _) = offer(staged(file), &[b"twenty bytes of fil!"], None);
    assert_eq!(
        holds.keep_published(published).await,
        Err(CollectionRefusal::Mismatch)
    );
    for described in [
        nessa_protocol::lease::StagedArtifact {
            digest: "not-a-digest".into(),
            ..staged(file)
        },
        nessa_protocol::lease::StagedArtifact {
            digest: format!("sha256:{}", staged(file).digest),
            ..staged(file)
        },
        nessa_protocol::lease::StagedArtifact {
            media_type: "PDF".into(),
            ..staged(file)
        },
    ] {
        let (published, read) = offer(described.clone(), &[file], None);
        assert_eq!(
            holds.keep_published(published).await,
            Err(CollectionRefusal::Invalid),
            "{described:?}"
        );
        assert!(!read.load(Ordering::SeqCst), "{described:?}");
    }
    assert!(fixture.store.held().is_empty());

    let (published, _) = offer(staged(file), &[&file[..7], &file[7..]], None);
    assert_eq!(
        holds.keep_published(published).await,
        Ok(ArtifactKept::Held)
    );
    assert_eq!(fixture.store.held()[0].lease(), Some("lease-1"));
    let (published, read) = offer(staged(file), &[file], None);
    assert_eq!(
        holds.keep_published(published).await,
        Ok(ArtifactKept::AlreadyHeld)
    );
    assert!(!read.load(Ordering::SeqCst));
}
