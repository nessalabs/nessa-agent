//! A user message's images reach the agent only when every gate agrees: the
//! model, a configured byte source, and the agent's own advertised capability.
use super::support::*;
use crate::domain::common::value_objects::Sha256Digest;
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::atomic::AtomicUsize, sync::atomic::Ordering};

/// A byte source answering from a fixed table and counting every read.
#[derive(Default)]
struct FixedImages {
    answers: HashMap<Sha256Digest, Result<Vec<u8>, UserImageError>>,
    reads: AtomicUsize,
}
impl UserImageSource for FixedImages {
    fn read(&self, image: ImageReference) -> UserImageFuture<'_> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers.get(&image.digest()).cloned();
        Box::pin(async move { answer.unwrap_or(Err(UserImageError::Missing)) })
    }
}

fn reference(bytes: &[u8], media_type: ImageMediaType) -> ImageReference {
    let digest = Sha256Digest::from_bytes(Sha256::digest(bytes).into());
    ImageReference::new(digest, media_type, bytes.len() as u64).unwrap()
}

fn message(id: &str, text: Option<&str>, images: Vec<ImageReference>) -> ExecutionRequest {
    ExecutionRequest {
        execution_id: ExecutionId::new(id).unwrap(),
        user_message: UserMessage::new(text.map(|text| PromptText::new(text).unwrap()), images)
            .unwrap(),
        estimated_input_tokens: 10,
        reserved_output_tokens: 100,
    }
}

/// The shared fixture with a model that lists image input, and an optional source.
fn image_provider(mode: &str, source: Option<Arc<FixedImages>>) -> (TempDir, ClaudeAcpProvider) {
    let (root, mut config, _) = test_acp_configuration(mode, 32);
    config.execution_timeout = None;
    config.images = source.map(|source| source as Arc<dyn UserImageSource>);
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "exact-fixture-model".into(),
        display_name: "Fixture".into(),
        input: ModalitiesDto {
            image: true,
            ..text
        },
        output: text,
        tool_use: true,
        reasoning: true,
        max_context_window_tokens: 1000,
        max_output_tokens: 200,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let provider = ClaudeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();
    (root, provider)
}

async fn close(opened: &OpenedProviderSession) {
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

fn observed(root: &TempDir, name: &str) -> Option<serde_json::Value> {
    std::fs::read_to_string(root.path().join(name))
        .ok()
        .map(|text| serde_json::from_str(&text).unwrap())
}

#[tokio::test]
async fn an_advertising_agent_receives_text_then_images_in_attachment_order() {
    let _slot = process_test_slot().await;
    let (png, jpeg) = (b"first image".as_slice(), b"second".as_slice());
    let (first, second) = (
        reference(png, ImageMediaType::Png),
        reference(jpeg, ImageMediaType::Jpeg),
    );
    let source = Arc::new(FixedImages {
        answers: HashMap::from([
            (first.digest(), Ok(png.to_vec())),
            (second.digest(), Ok(jpeg.to_vec())),
        ]),
        ..Default::default()
    });
    let (root, provider) = image_provider("image-input", Some(source.clone()));
    let opened = provider.open(None).await.unwrap();
    assert!(opened.session.operation_capabilities().image_input);

    let sent = message("both", Some("what is this?"), vec![second, first]);
    let result = opened.session.execute(sent).await.into_result();
    assert_eq!(result, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        observed(&root, "prompt-observed").unwrap(),
        serde_json::json!([
            {"type": "text", "text": "what is this?"},
            {"type": "image", "mimeType": "image/jpeg", "data": STANDARD.encode(jpeg)},
            {"type": "image", "mimeType": "image/png", "data": STANDARD.encode(png)},
        ])
    );

    // An image alone is a whole message: no empty text block is invented.
    let alone = message("alone", None, vec![first]);
    let result = opened.session.execute(alone).await.into_result();
    assert_eq!(result, Ok(ExecutionOutcome::Completed));
    assert_eq!(
        observed(&root, "prompt-observed").unwrap(),
        serde_json::json!([
            {"type": "image", "mimeType": "image/png", "data": STANDARD.encode(png)},
        ])
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 3);
    close(&opened).await;
}

#[tokio::test]
async fn an_agent_that_did_not_advertise_images_is_sent_nothing() {
    let _slot = process_test_slot().await;
    let bytes = b"image".as_slice();
    let image = reference(bytes, ImageMediaType::Png);
    let source = Arc::new(FixedImages {
        answers: HashMap::from([(image.digest(), Ok(bytes.to_vec()))]),
        ..Default::default()
    });
    let (root, provider) = image_provider("plain", Some(source.clone()));
    // The binding can deliver images, so admission allows them...
    let opened = provider.open(None).await.unwrap();
    assert!(opened.session.capabilities().features().input().image());
    // ...but this agent never agreed to receive one.
    assert!(!opened.session.operation_capabilities().image_input);

    let refused = message("refused", Some("look"), vec![image]);
    let result = opened.session.execute(refused).await.into_result();
    assert!(
        matches!(result, Err(AgentError::Unsupported(_))),
        "{result:?}"
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    assert_eq!(observed(&root, "prompt-observed"), None);

    // The refusal dispatched nothing, so the context is still usable.
    let result = opened.session.execute(prompt("text")).await.into_result();
    assert_eq!(result, Ok(ExecutionOutcome::Completed));
    close(&opened).await;
}

#[tokio::test]
async fn a_binding_without_a_byte_source_refuses_images_at_admission() {
    let _slot = process_test_slot().await;
    let (root, provider) = image_provider("image-input", None);
    let opened = provider.open(None).await.unwrap();
    assert!(!opened.session.capabilities().features().input().image());
    // The agent advertised images, but this process has nowhere to read them from.
    assert!(!opened.session.operation_capabilities().image_input);

    let image = reference(b"image", ImageMediaType::Webp);
    let result = opened
        .session
        .execute(message("refused", Some("look"), vec![image]))
        .await
        .into_result();
    assert!(
        matches!(result, Err(AgentError::InvalidInput(_))),
        "{result:?}"
    );
    assert_eq!(observed(&root, "prompt-observed"), None);
    close(&opened).await;
}

#[tokio::test]
async fn an_image_that_cannot_be_supplied_intact_sends_nothing_and_keeps_the_context() {
    let _slot = process_test_slot().await;
    let bytes = b"what the user attached".as_slice();
    let missing = reference(b"never stored", ImageMediaType::Png);
    let unavailable = reference(b"store is down", ImageMediaType::Png);
    let substituted = reference(bytes, ImageMediaType::Png);
    let truncated = reference(b"longer than returned", ImageMediaType::Gif);
    let intact = reference(b"intact", ImageMediaType::Png);
    let source = Arc::new(FixedImages {
        answers: HashMap::from([
            (unavailable.digest(), Err(UserImageError::Unavailable)),
            // Same length, different content.
            (substituted.digest(), Ok(b"what the user ATTACHED".to_vec())),
            (truncated.digest(), Ok(b"longer".to_vec())),
            (intact.digest(), Ok(b"intact".to_vec())),
        ]),
        ..Default::default()
    });
    let (root, provider) = image_provider("image-input", Some(source));
    let opened = provider.open(None).await.unwrap();

    for (id, image, expected) in [
        ("missing", missing, UserImageError::Missing),
        ("unavailable", unavailable, UserImageError::Unavailable),
        ("substituted", substituted, UserImageError::Mismatch),
        ("truncated", truncated, UserImageError::Mismatch),
    ] {
        // A good image before the bad one must not be sent on its own either.
        let refused = message(id, Some("look"), vec![intact, image]);
        let result = opened.session.execute(refused).await.into_result();
        assert_eq!(result, Err(AgentError::UserImage(expected)), "{id}");
        assert_eq!(observed(&root, "prompt-observed"), None, "{id}");
    }

    let result = opened
        .session
        .execute(message("intact", None, vec![intact]))
        .await
        .into_result();
    assert_eq!(result, Ok(ExecutionOutcome::Completed));
    close(&opened).await;
}

#[tokio::test]
async fn steering_carries_images_the_same_way_a_prompt_does() {
    let _slot = process_test_slot().await;
    let bytes = b"steering image".as_slice();
    let image = reference(bytes, ImageMediaType::Png);
    let source = Arc::new(FixedImages {
        answers: HashMap::from([(image.digest(), Ok(bytes.to_vec()))]),
        ..Default::default()
    });
    let (root, provider) = image_provider("steering-image-injected", Some(source));
    let mut opened = provider.open(None).await.unwrap();
    let active = start(&opened, "first").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:first"))
    );

    let missing = reference(b"never stored", ImageMediaType::Png);
    let failure = opened
        .session
        .steer(
            ExecutionId::new("first").unwrap(),
            message("bad", Some("adjust"), vec![missing]),
        )
        .await
        .unwrap_err();
    assert_eq!(
        failure.error(),
        &AgentError::UserImage(UserImageError::Missing)
    );
    assert_eq!(observed(&root, "steering-observed"), None);

    let outcome = opened
        .session
        .steer(
            ExecutionId::new("first").unwrap(),
            message("good", Some("adjust"), vec![image]),
        )
        .await;
    assert_eq!(outcome.unwrap(), SteeringOutcome::Injected);
    assert_eq!(
        observed(&root, "steering-observed").unwrap(),
        serde_json::json!([
            {"type": "text", "text": "adjust"},
            {"type": "image", "mimeType": "image/png", "data": STANDARD.encode(bytes)},
        ])
    );
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
    close(&opened).await;
}
