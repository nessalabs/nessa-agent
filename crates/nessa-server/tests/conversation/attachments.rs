//! Messages that refer to images, through the real SDK Agent: what is refused
//! before acceptance, what a view echoes, and what closing lets go of.
use super::{
    AttachmentReleaseCause, ConversationAttachment, ConversationCaller, ConversationDependencies,
    ConversationError, ConversationLimits, ConversationMessageStatus, ConversationService,
    SubmissionMode, SubmittedImage,
};
use crate::{
    conversation::domain::ConversationId,
    conversation_test_support::{
        image_fixture, AcceptingCreationAudit, MemoryAttachments, Provider, ProviderFactory,
        TestClock,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::agents::AgentError,
    domain::{
        agent_execution::prompts::ImageReference,
        common::value_objects::{ImageMediaType, Sha256Digest},
    },
};
use std::sync::{atomic::Ordering, Arc};
use tokio::sync::oneshot;

fn new_id() -> ConversationId {
    ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap()
}
fn caller(surface: &str, action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: surface.into(),
        action_id: action.into(),
    }
}
fn image(byte: u8) -> ImageReference {
    ImageReference::new(
        Sha256Digest::from_bytes([byte; 32]),
        ImageMediaType::Png,
        1024,
    )
    .unwrap()
}
fn submitted(image: &ImageReference) -> SubmittedImage {
    SubmittedImage {
        digest: image.digest().to_string(),
        media_type: image.media_type().as_str().into(),
        size: image.size(),
    }
}
fn echoed(image: &ImageReference) -> ConversationAttachment {
    ConversationAttachment::from(image)
}
/// A conversation whose agent takes images and which holds `held`.
async fn conversation_holding(
    held: &[ImageReference],
) -> (
    ConversationService,
    Arc<ProviderFactory>,
    Arc<MemoryAttachments>,
    ConversationId,
) {
    let attachments = Arc::new(MemoryAttachments::default());
    let (service, provider, _, _) = image_fixture(true, Some(attachments.clone()));
    let id = new_id();
    service
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    for image in held {
        attachments.held.lock().unwrap().push((
            OrganizationId::new("org").unwrap(),
            id.clone(),
            *image,
        ));
    }
    (service, provider, attachments, id)
}
async fn send(
    service: &ConversationService,
    id: &ConversationId,
    execution: &str,
    text: &str,
    images: &[ImageReference],
) -> Result<(), ConversationError> {
    service
        .submit(
            id.clone(),
            caller("panel", execution),
            execution.into(),
            text.into(),
            images.iter().map(submitted).collect(),
            SubmissionMode::Queue,
        )
        .await
        .map(|_| ())
}
async fn completed(service: &ConversationService, id: &ConversationId, count: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let view = service
                .read(id.clone(), caller("panel", "read"))
                .await
                .unwrap();
            if view
                .messages
                .iter()
                .filter(|message| message.status == ConversationMessageStatus::Completed)
                .count()
                == count
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn an_image_is_refused_when_the_agent_or_the_gateway_takes_none() {
    let attachments = Arc::new(MemoryAttachments::default());
    // An agent that did not agree to images, and a gateway that keeps no uploads.
    for (image_input, port) in [(false, Some(attachments.clone() as Arc<_>)), (true, None)] {
        let (service, provider, _, _) = image_fixture(image_input, port);
        let id = new_id();
        service
            .create(id.clone(), caller("panel", "create"))
            .await
            .unwrap();
        attachments.held.lock().unwrap().push((
            OrganizationId::new("org").unwrap(),
            id.clone(),
            image(1),
        ));
        assert!(matches!(
            send(&service, &id, "first", "look", &[image(1)]).await,
            Err(ConversationError::ImagesUnsupported)
        ));
        let view = service
            .read(id.clone(), caller("panel", "read"))
            .await
            .unwrap();
        assert!(!view.capabilities.image_input);
        assert!(view.messages.is_empty() && view.pending.is_empty());
        assert!(provider.executions.lock().unwrap().is_empty());
        // Text alone is still a message, under the identifier the refusal did not use up.
        send(&service, &id, "first", "look", &[]).await.unwrap();
        service.shutdown().await.unwrap();
    }
    // Refused for what the agent is, before anyone was asked what is held.
    assert_eq!(attachments.asked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_image_this_conversation_does_not_hold_is_refused_before_anything_is_admitted() {
    let (service, provider, attachments, id) = conversation_holding(&[image(1)]).await;
    for images in [vec![image(2)], vec![image(1), image(2)]] {
        assert!(matches!(
            send(&service, &id, "first", "look", &images).await,
            Err(ConversationError::AttachmentNotFound)
        ));
    }
    // Held by another conversation of the same owner is not held by this one.
    let other = new_id();
    service
        .create(other.clone(), caller("panel", "create-other"))
        .await
        .unwrap();
    assert!(matches!(
        send(&service, &other, "first", "look", &[image(1)]).await,
        Err(ConversationError::AttachmentNotFound)
    ));
    assert!(attachments.asked.load(Ordering::SeqCst) >= 3);

    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert!(view.capabilities.image_input);
    assert!(view.messages.is_empty() && view.pending.is_empty());
    assert!(provider.executions.lock().unwrap().is_empty());
    // Nothing was admitted, so the identifier is free and conflicts with nothing.
    send(&service, &id, "first", "look", &[image(1)])
        .await
        .unwrap();
    completed(&service, &id, 1).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_view_echoes_a_turns_images_while_it_waits_once_it_ran_and_after_a_restart() {
    let attachments = Arc::new(MemoryAttachments::default());
    let (service, provider, repository, storage) = image_fixture(true, Some(attachments.clone()));
    let id = new_id();
    service
        .create(id.clone(), caller("panel", "create"))
        .await
        .unwrap();
    for held in [image(1), image(2), image(3)] {
        attachments.held.lock().unwrap().push((
            OrganizationId::new("org").unwrap(),
            id.clone(),
            held,
        ));
    }
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    send(&service, &id, "first", "two images", &[image(1), image(2)])
        .await
        .unwrap();
    provider.execution_started.notified().await;
    // An image alone is a message: blank text is no text.
    send(&service, &id, "second", " \n", &[image(3)])
        .await
        .unwrap();

    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    let waiting = view
        .pending
        .iter()
        .find(|pending| pending.execution_id == "second")
        .unwrap();
    assert_eq!(waiting.attachments, [echoed(&image(3))]);
    assert_eq!(waiting.text, "");

    release.send(()).unwrap();
    completed(&service, &id, 2).await;
    let view = service
        .read(id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert_eq!(
        view.messages[0].attachments,
        [echoed(&image(1)), echoed(&image(2))]
    );
    assert_eq!(view.messages[1].attachments, [echoed(&image(3))]);
    assert_eq!(view.messages[1].user_text, "");
    // The agent was handed the references, in the order they were attached.
    assert_eq!(
        *provider.images.lock().unwrap(),
        [image(1), image(2), image(3)]
    );

    service.shutdown().await.unwrap();
    drop(service);
    tokio::task::yield_now().await;
    let restored = ConversationService::new(
        ConversationDependencies {
            provider: Arc::new(Provider(provider.clone())),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            attachments: Some(attachments),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let view = restored.read(id, caller("phone", "read")).await.unwrap();
    assert_eq!(
        view.messages[0].attachments,
        [echoed(&image(1)), echoed(&image(2))]
    );
    assert_eq!(view.messages[1].attachments, [echoed(&image(3))]);
    assert_eq!(provider.executions.lock().unwrap().len(), 2);
    restored.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_retry_with_the_same_images_recovers_and_with_other_images_conflicts() {
    let (service, provider, _, id) = conversation_holding(&[image(1), image(2)]).await;
    send(&service, &id, "first", "look", &[image(1)])
        .await
        .unwrap();
    completed(&service, &id, 1).await;

    send(&service, &id, "first", "look", &[image(1)])
        .await
        .unwrap();
    for different in [vec![image(2)], vec![image(1), image(2)], vec![]] {
        assert!(matches!(
            send(&service, &id, "first", "look", &different).await,
            Err(ConversationError::Agent(AgentError::SubmissionConflict))
        ));
    }
    assert_eq!(*provider.executions.lock().unwrap(), ["first"]);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn closing_lets_go_of_the_conversations_uploads_in_the_closers_name() {
    let (service, _, attachments, id) = conversation_holding(&[image(1)]).await;
    service
        .close(id.clone(), caller("phone", "close-1"))
        .await
        .unwrap();

    let releases = attachments.releases.lock().unwrap();
    let [release] = releases.as_slice() else {
        panic!("one release expected, got {}", releases.len())
    };
    assert_eq!(release.organization_id.as_str(), "org");
    assert_eq!(release.conversation_id, id);
    assert_eq!(release.cause, AttachmentReleaseCause::ConversationClosed);
    assert_eq!(release.initiator_principal_id.as_str(), "person");
    assert_eq!(release.initiator_surface_id, "phone");
    assert_eq!(release.correlation_id, "close-1");
    assert!(attachments.held.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_release_that_fails_is_reported_without_hiding_an_agent_that_did_not_close() {
    // The conversation closed; only letting go of its uploads did not complete.
    let (service, _, attachments, id) = conversation_holding(&[image(1)]).await;
    attachments.release_fails.store(true, Ordering::SeqCst);
    let closed = service.close(id.clone(), caller("panel", "close-1")).await;
    let Err(ConversationError::AttachmentRelease(cause)) = closed else {
        panic!("the release failure must be reported, got {closed:?}")
    };
    assert!(matches!(*cause, ConversationError::Audit));
    assert_eq!(attachments.releases.lock().unwrap().len(), 1);

    // Both failed. The agent's failure is the one named, and the uploads were
    // still let go rather than waiting on an agent that will not close.
    let (service, provider, attachments, id) = conversation_holding(&[image(1)]).await;
    attachments.release_fails.store(true, Ordering::SeqCst);
    *provider.close_failure.lock().unwrap() = Some(AgentError::Deadline);
    assert!(matches!(
        service.close(id.clone(), caller("panel", "close-1")).await,
        Err(ConversationError::Agent(_))
    ));
    assert_eq!(attachments.releases.lock().unwrap().len(), 1);
    assert!(attachments.held.lock().unwrap().is_empty());
}
