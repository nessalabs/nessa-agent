//! A user message's images reach the agent only when every gate agrees: the
//! model, a configured byte source, and the agent's own advertised capability.
//!
//! Reading the bytes never holds up the context: a source that stalls or
//! panics costs one message a typed failure and nothing else.
use super::support::*;
use crate::application::dto::ImageInputLimitsDto;
use crate::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use crate::infrastructure::acp::executions::{
    prompt_content::IMAGE_READ_TIMEOUT, steering::RESPONSE_TIMEOUT,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::pending,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

/// How a [`FixedImages`] source behaves for a digest it has no answer for.
#[derive(Default)]
enum Otherwise {
    /// Nothing is stored under it.
    #[default]
    Missing,
    /// The read begins, says so, and never finishes.
    Stalls(mpsc::UnboundedSender<()>),
    /// The read panics when it is polled.
    Panics,
}

/// A byte source answering from a fixed table and counting every read.
#[derive(Default)]
struct FixedImages {
    answers: HashMap<Sha256Digest, Result<Vec<u8>, UserImageError>>,
    otherwise: Otherwise,
    reads: AtomicUsize,
}
impl UserImageSource for FixedImages {
    fn read(&self, image: ImageReference) -> UserImageFuture<'_> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let answer = self.answers.get(&image.digest()).cloned();
        Box::pin(async move {
            match (answer, &self.otherwise) {
                (Some(answer), _) => answer,
                (None, Otherwise::Missing) => Err(UserImageError::Missing),
                (None, Otherwise::Stalls(began)) => {
                    began.send(()).unwrap();
                    pending().await
                }
                (None, Otherwise::Panics) => panic!("fixture image source panicked"),
            }
        })
    }
}
fn stalling() -> (Arc<FixedImages>, mpsc::UnboundedReceiver<()>) {
    let (began, begins) = mpsc::unbounded_channel();
    let source = Arc::new(FixedImages {
        otherwise: Otherwise::Stalls(began),
        ..Default::default()
    });
    (source, begins)
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
    image_provider_with(mode, source, true)
}

fn image_provider_with(
    mode: &str,
    source: Option<Arc<FixedImages>>,
    limits_recorded: bool,
) -> (TempDir, ClaudeAcpProvider) {
    let (root, mut config, _) = test_acp_configuration(mode, 32);
    config.execution_timeout = None;
    image_provider_from(root, config, source, limits_recorded)
}

fn image_provider_from(
    root: TempDir,
    mut config: AcpConfig,
    source: Option<Arc<FixedImages>>,
    limits_recorded: bool,
) -> (TempDir, ClaudeAcpProvider) {
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
        image_input: limits_recorded.then(|| ImageInputLimitsDto {
            media_types: vec!["image/png".into(), "image/jpeg".into()],
            max_encoded_bytes: 5_000_000,
            max_edge_px: 8000,
            many_images_max_edge_px: 2000,
            native_long_edge_px: 1568,
        }),
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
    assert_eq!(
        result,
        Err(AgentError::ImageInputRefused(
            ImageInputRefusal::AgentDoesNotAccept
        ))
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
    let truncated = reference(b"longer than returned", ImageMediaType::Png);
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

#[tokio::test]
async fn a_model_without_recorded_image_limits_is_offered_no_images() {
    let _slot = process_test_slot().await;
    // The model lists image input, a source is configured, and the agent
    // advertises images. Nothing says what an acceptable image is, so nothing
    // could have prepared one: the binding offers none.
    let source = Arc::new(FixedImages::default());
    let (root, provider) = image_provider_with("image-input", Some(source.clone()), false);
    let opened = provider.open(None).await.unwrap();
    assert!(!opened.session.capabilities().features().input().image());
    assert_eq!(opened.session.capabilities().image_input(), None);

    let image = reference(b"image", ImageMediaType::Png);
    let result = opened
        .session
        .execute(message("refused", Some("look"), vec![image]))
        .await
        .into_result();
    assert!(
        matches!(result, Err(AgentError::InvalidInput(_))),
        "{result:?}"
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    assert_eq!(observed(&root, "prompt-observed"), None);
    close(&opened).await;
}

#[tokio::test]
async fn an_image_outside_the_models_limits_or_the_frame_is_refused_without_a_read() {
    let _slot = process_test_slot().await;
    let source = Arc::new(FixedImages::default());
    let (root, provider) = image_provider("image-input", Some(source.clone()));
    let opened = provider.open(None).await.unwrap();

    // The fixture model lists PNG and JPEG, and its frames hold 8192 bytes.
    let webp = reference(b"image", ImageMediaType::Webp);
    let result = opened
        .session
        .execute(message("webp", Some("look"), vec![webp]))
        .await
        .into_result();
    assert_eq!(
        result,
        Err(AgentError::ImageInputRefused(ImageInputRefusal::MediaType(
            ImageMediaType::Webp
        )))
    );
    let large = reference(&[7; 6000], ImageMediaType::Png);
    for steering in [false, true] {
        let sent = message("large", Some("look"), vec![large]);
        let result = if steering {
            opened
                .session
                .steer(ExecutionId::new("none").unwrap(), sent)
                .await
                .map(|_| ExecutionOutcome::Completed)
                .map_err(|failure| failure.into_error())
        } else {
            opened.session.execute(sent).await.into_result()
        };
        assert!(
            matches!(
                result,
                Err(AgentError::MessageTooLarge { encoded_bytes, max_bytes: 8192 })
                    if encoded_bytes > 8000
            ),
            "{result:?}"
        );
    }
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    assert_eq!(observed(&root, "prompt-observed"), None);
    close(&opened).await;
}

#[tokio::test]
async fn a_source_that_never_answers_does_not_keep_the_context_from_closing() {
    let _slot = process_test_slot().await;
    let (source, mut begins) = stalling();
    let (root, provider) = image_provider("image-input", Some(source));
    let opened = provider.open(None).await.unwrap();
    let session = opened.session.clone();
    let stalled = message(
        "stalled",
        Some("look"),
        vec![reference(b"x", ImageMediaType::Png)],
    );
    let running = tokio::spawn(async move { session.execute(stalled).await.into_result() });
    begins.recv().await.unwrap();

    // The read is still outstanding, and close completes anyway.
    timeout(Duration::from_secs(5), close(&opened))
        .await
        .expect("a stalled image read blocked close");
    let result = timeout(Duration::from_secs(5), running)
        .await
        .expect("a stalled image read outlived close")
        .unwrap();
    assert_eq!(result, Err(AgentError::Closed));
    assert_eq!(observed(&root, "prompt-observed"), None);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn a_source_that_never_answers_is_unavailable_at_the_read_bound_and_keeps_the_context() {
    let _slot = process_test_slot().await;
    let (source, mut begins) = stalling();
    // No execution timeout: the read still has a bound of its own.
    let (root, provider) = image_provider("image-input", Some(source));
    let opened = provider.open(None).await.unwrap();
    let session = opened.session.clone();
    let stalled = message(
        "stalled",
        Some("look"),
        vec![reference(b"x", ImageMediaType::Png)],
    );
    let running = tokio::spawn(async move { session.execute(stalled).await.into_result() });
    begins.recv().await.unwrap();

    // Only the simulated read deadline is advanced; real time resumes before
    // anything waits on the agent process again.
    tokio::time::pause();
    tokio::time::advance(IMAGE_READ_TIMEOUT).await;
    tokio::time::resume();
    let result = timeout(Duration::from_secs(5), running).await.unwrap();
    assert_eq!(
        result.unwrap(),
        Err(AgentError::UserImage(UserImageError::Unavailable))
    );
    assert_eq!(observed(&root, "prompt-observed"), None);

    let result = opened.session.execute(prompt("text")).await.into_result();
    assert_eq!(result, Ok(ExecutionOutcome::Completed));
    close(&opened).await;
}

#[tokio::test]
async fn a_shorter_execution_timeout_shortens_the_read_bound() {
    let _slot = process_test_slot().await;
    let (source, mut begins) = stalling();
    let (root, mut config, _) = test_acp_configuration("image-input", 32);
    let limit = Duration::from_secs(2);
    assert!(limit < IMAGE_READ_TIMEOUT);
    config.execution_timeout = Some(limit);
    let (root, provider) = image_provider_from(root, config, Some(source), true);
    let opened = provider.open(None).await.unwrap();
    let session = opened.session.clone();
    let stalled = message(
        "stalled",
        Some("look"),
        vec![reference(b"x", ImageMediaType::Png)],
    );
    let running = tokio::spawn(async move { session.execute(stalled).await.into_result() });
    begins.recv().await.unwrap();

    tokio::time::pause();
    tokio::time::advance(limit).await;
    tokio::time::resume();
    let result = timeout(Duration::from_secs(5), running).await.unwrap();
    assert_eq!(
        result.unwrap(),
        Err(AgentError::UserImage(UserImageError::Unavailable))
    );
    assert_eq!(observed(&root, "prompt-observed"), None);
    close(&opened).await;
}

#[tokio::test]
async fn a_source_that_panics_costs_one_message_and_not_the_worker() {
    let _slot = process_test_slot().await;
    let source = Arc::new(FixedImages {
        otherwise: Otherwise::Panics,
        ..Default::default()
    });
    let (root, provider) = image_provider("image-input", Some(source));
    let opened = provider.open(None).await.unwrap();
    let panicking = message(
        "panics",
        Some("look"),
        vec![reference(b"x", ImageMediaType::Png)],
    );
    let result = opened.session.execute(panicking).await.into_result();
    assert_eq!(
        result,
        Err(AgentError::UserImage(UserImageError::Unavailable))
    );
    assert_eq!(observed(&root, "prompt-observed"), None);

    // The same process is still there and still answers.
    let result = opened.session.execute(prompt("text")).await.into_result();
    assert_eq!(result, Ok(ExecutionOutcome::Completed));
    close(&opened).await;
}

#[tokio::test]
async fn a_steering_read_that_never_answers_does_not_hold_up_the_active_execution() {
    let _slot = process_test_slot().await;
    let (source, mut begins) = stalling();
    let (root, provider) = image_provider("steering-image-injected", Some(source));
    let mut opened = provider.open(None).await.unwrap();
    let active = start(&opened, "first").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:first"))
    );
    let session = opened.session.clone();
    let stalled = message(
        "stalled",
        Some("adjust"),
        vec![reference(b"x", ImageMediaType::Png)],
    );
    let steering = tokio::spawn(async move {
        session
            .steer(ExecutionId::new("first").unwrap(), stalled)
            .await
    });
    begins.recv().await.unwrap();

    // With that read outstanding the worker still takes other commands and
    // still hears the agent: a second steering is injected, and the active
    // execution settles.
    let outcome = timeout(
        Duration::from_secs(5),
        opened
            .session
            .steer(ExecutionId::new("first").unwrap(), prompt("text steering")),
    )
    .await
    .expect("a stalled steering read blocked the worker");
    assert_eq!(outcome.unwrap(), SteeringOutcome::Injected);
    let settled = timeout(Duration::from_secs(5), active)
        .await
        .expect("a stalled steering read kept the active execution from settling");
    assert_eq!(settled.unwrap(), Ok(ExecutionOutcome::Completed));
    assert!(!steering.is_finished());
    assert_eq!(
        observed(&root, "steering-observed").unwrap(),
        serde_json::json!([{"type": "text", "text": "text steering"}])
    );

    // Close abandons the read and tells its caller so.
    timeout(Duration::from_secs(5), close(&opened))
        .await
        .expect("a stalled steering read blocked close");
    let failure = timeout(Duration::from_secs(5), steering)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(failure.error(), &AgentError::Closed);
}

#[tokio::test]
async fn a_steering_read_that_never_answers_is_unavailable_at_the_steering_bound() {
    let _slot = process_test_slot().await;
    let (source, mut begins) = stalling();
    let (root, provider) = image_provider("steering-image-injected", Some(source));
    let mut opened = provider.open(None).await.unwrap();
    let active = start(&opened, "first").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running:first"))
    );
    let session = opened.session.clone();
    let stalled = message(
        "stalled",
        Some("adjust"),
        vec![reference(b"x", ImageMediaType::Png)],
    );
    let steering = tokio::spawn(async move {
        session
            .steer(ExecutionId::new("first").unwrap(), stalled)
            .await
    });
    begins.recv().await.unwrap();

    assert!(RESPONSE_TIMEOUT < IMAGE_READ_TIMEOUT);
    tokio::time::pause();
    tokio::time::advance(RESPONSE_TIMEOUT).await;
    tokio::time::resume();
    let failure = timeout(Duration::from_secs(5), steering)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        failure.error(),
        &AgentError::UserImage(UserImageError::Unavailable)
    );
    assert_eq!(observed(&root, "steering-observed"), None);

    // Nothing was sent, so the active execution is untouched and still steerable.
    assert!(!active.is_finished());
    let outcome = opened
        .session
        .steer(ExecutionId::new("first").unwrap(), prompt("text steering"))
        .await;
    assert_eq!(outcome.unwrap(), SteeringOutcome::Injected);
    assert_eq!(active.await.unwrap(), Ok(ExecutionOutcome::Completed));
    close(&opened).await;
}
