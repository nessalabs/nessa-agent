//! Reading a message's images is bounded, abandoned on close, and survives a
//! source that panics; the frame figure used at admission covers the real frame.
use super::*;
use crate::application::agent_execution::providers::UserImageFuture;
use crate::domain::{
    agent_execution::prompts::PromptText,
    common::value_objects::{ImageMediaType, Sha256Digest},
};
use crate::infrastructure::json_rpc;
use std::{
    future::pending,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::{sync::oneshot, time::Instant};

fn reference(bytes: &[u8], media_type: ImageMediaType) -> ImageReference {
    let digest = Sha256Digest::from_bytes(Sha256::digest(bytes).into());
    ImageReference::new(digest, media_type, bytes.len() as u64).unwrap()
}
fn sized(size: u64) -> ImageReference {
    ImageReference::new(
        Sha256Digest::from_bytes([7; 32]),
        ImageMediaType::Jpeg,
        size,
    )
    .unwrap()
}
fn message(text: Option<&str>, images: Vec<ImageReference>) -> UserMessage {
    UserMessage::new(text.map(|text| PromptText::new(text).unwrap()), images).unwrap()
}
fn figure(message: &UserMessage) -> u64 {
    match fits_one_frame(message, 0) {
        Err(AgentError::MessageTooLarge { encoded_bytes, .. }) => encoded_bytes,
        other => panic!("{other:?}"),
    }
}

/// Answers the same bytes for every reference.
struct Always(Vec<u8>);
impl UserImageSource for Always {
    fn read(&self, _: ImageReference) -> UserImageFuture<'_> {
        let bytes = self.0.clone();
        Box::pin(async move { Ok(bytes) })
    }
}

/// Never answers. Says when a read began and when its future was dropped.
struct Stalled {
    started: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    dropped: Arc<AtomicBool>,
}
struct SetOnDrop(Arc<AtomicBool>);
impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl UserImageSource for Stalled {
    fn read(&self, _: ImageReference) -> UserImageFuture<'_> {
        let started = self.started.lock().unwrap().take();
        let dropped = SetOnDrop(self.dropped.clone());
        Box::pin(async move {
            let _dropped = dropped;
            if let Some(started) = started {
                let _ = started.send(());
            }
            pending().await
        })
    }
}
fn stalled() -> (Stalled, oneshot::Receiver<()>, Arc<AtomicBool>) {
    let (started, began) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let source = Stalled {
        started: std::sync::Mutex::new(Some(started)),
        dropped: dropped.clone(),
    };
    (source, began, dropped)
}

/// Panics either when asked for a read or when that read is first polled.
struct Panics {
    when_polled: bool,
    reads: AtomicUsize,
}
impl UserImageSource for Panics {
    fn read(&self, _: ImageReference) -> UserImageFuture<'_> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if !self.when_polled {
            panic!("fixture source panicked when asked");
        }
        Box::pin(async { panic!("fixture source panicked when polled") })
    }
}

#[tokio::test(start_paused = true)]
async fn a_source_that_never_answers_is_unavailable_exactly_at_the_bound() {
    let (source, _began, dropped) = stalled();
    let sent = message(Some("look"), vec![reference(b"image", ImageMediaType::Png)]);
    let began_at = Instant::now();
    let result = read_images(Some(&source), &sent, IMAGE_READ_TIMEOUT, pending()).await;
    assert_eq!(
        result.err(),
        Some(AgentError::UserImage(UserImageError::Unavailable))
    );
    assert_eq!(began_at.elapsed(), IMAGE_READ_TIMEOUT);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the read outlived its bound"
    );
}

#[tokio::test]
async fn stopping_abandons_a_read_that_never_answers() {
    let (source, began, dropped) = stalled();
    let sent = message(None, vec![reference(b"image", ImageMediaType::Png)]);
    let (stop, stopped) = oneshot::channel::<()>();
    let reading = read_images(Some(&source), &sent, IMAGE_READ_TIMEOUT, async {
        let _ = stopped.await;
    });
    let stopping = async {
        began.await.unwrap();
        assert!(!dropped.load(Ordering::SeqCst));
        stop.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(reading, stopping);
    assert_eq!(result.err(), Some(AgentError::Closed));
    assert!(dropped.load(Ordering::SeqCst), "the read outlived the stop");
}

#[tokio::test]
async fn a_source_that_panics_is_a_typed_failure_not_an_unwinding_caller() {
    for when_polled in [false, true] {
        let source = Panics {
            when_polled,
            reads: AtomicUsize::new(0),
        };
        let sent = message(
            Some("look"),
            vec![
                reference(b"first", ImageMediaType::Png),
                reference(b"second", ImageMediaType::Png),
            ],
        );
        let result = read_images(Some(&source), &sent, IMAGE_READ_TIMEOUT, pending()).await;
        assert_eq!(
            result.err(),
            Some(AgentError::UserImage(UserImageError::Unavailable)),
            "when_polled={when_polled}"
        );
        // Nothing after the failed image is asked for.
        assert_eq!(source.reads.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn a_message_without_images_touches_neither_the_source_nor_the_clock() {
    let source = Panics {
        when_polled: false,
        reads: AtomicUsize::new(0),
    };
    let sent = message(Some("text"), Vec::new());
    // Already stopped, and no source at all: neither matters without an image.
    for source in [Some(&source as &dyn UserImageSource), None] {
        let blocks = read_images(source, &sent, IMAGE_READ_TIMEOUT, async {})
            .await
            .unwrap();
        assert_eq!(
            content_blocks(&sent, blocks).unwrap(),
            vec![json!({"type":"text","text":"text"})]
        );
    }
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn images_without_a_source_are_the_typed_refusal() {
    let sent = message(None, vec![reference(b"image", ImageMediaType::Png)]);
    let result = read_images(None, &sent, IMAGE_READ_TIMEOUT, pending()).await;
    assert_eq!(
        result.err(),
        Some(AgentError::ImageInputRefused(
            ImageInputRefusal::AgentDoesNotAccept
        ))
    );
}

#[tokio::test]
async fn an_answer_of_any_other_length_is_a_mismatch_even_when_its_digest_agrees() {
    // The reference carries the digest of the longer answer and a shorter
    // size, so only the length can tell them apart.
    let answer = b"longer than the reference says".to_vec();
    let digest = Sha256Digest::from_bytes(Sha256::digest(&answer).into());
    let claimed = ImageReference::new(digest, ImageMediaType::Png, 6).unwrap();
    let result = read_images(
        Some(&Always(answer)),
        &message(None, vec![claimed]),
        IMAGE_READ_TIMEOUT,
        pending(),
    )
    .await;
    assert_eq!(
        result.err(),
        Some(AgentError::UserImage(UserImageError::Mismatch))
    );
}

#[test]
fn blocks_for_another_message_are_refused_rather_than_sent() {
    let sent = message(Some("look"), vec![reference(b"image", ImageMediaType::Png)]);
    assert!(matches!(
        content_blocks(&sent, ImageBlocks::none()),
        Err(AgentError::Protocol(_))
    ));
}

#[test]
fn json_string_length_matches_the_serializer_for_every_kind_of_escape() {
    let every_control: String = (0_u8..0x20).map(char::from).collect();
    for text in [
        "plain",
        "quote \" and backslash \\",
        "\u{7f} is copied, and so is \u{80}, é, and 😀",
        every_control.as_str(),
    ] {
        assert_eq!(
            json_string_bytes(text),
            serde_json::to_string(text).unwrap().len() as u64,
            "{text:?}"
        );
    }
}

#[tokio::test]
async fn a_message_that_passes_the_frame_check_always_encodes_inside_the_frame() {
    // The worst case for everything the figure only allows for: the longest
    // request method, the largest identifier, and a session identifier of 256
    // bytes that each escape to six.
    let session = "\u{1}".repeat(256);
    let bytes = vec![0xab_u8; 1000];
    let image = reference(&bytes, ImageMediaType::Jpeg);
    for text in [None, Some("line\nbreak \"quoted\" \u{2} 😀")] {
        let sent = message(text, vec![image, image, image]);
        let blocks = read_images(
            Some(&Always(bytes.clone())),
            &sent,
            IMAGE_READ_TIMEOUT,
            pending(),
        )
        .await
        .unwrap();
        let frame = json_rpc::encode(
            json_rpc::request(
                i64::MAX,
                "_session/steering",
                json!({
                    "sessionId": session,
                    "prompt": content_blocks(&sent, blocks).unwrap(),
                    "_meta": {"steering":{"idleBehavior":"promptRequired"}}
                }),
            ),
            usize::MAX,
        )
        .unwrap();
        let figure = figure(&sent);
        assert!(
            frame.len() as u64 <= figure,
            "frame {} exceeds figure {figure}",
            frame.len()
        );
        // The figure is the bound itself: it fits exactly, and one byte less does not.
        assert_eq!(fits_one_frame(&sent, figure as usize), Ok(()));
        assert_eq!(
            fits_one_frame(&sent, figure as usize - 1),
            Err(AgentError::MessageTooLarge {
                encoded_bytes: figure,
                max_bytes: figure - 1,
            })
        );
    }
}

#[test]
fn the_largest_message_the_domain_allows_does_not_fit_the_largest_frame() {
    const LARGEST_FRAME: usize = 16 * 1024 * 1024;
    let images = vec![
        sized(ImageReference::MAX_BYTES),
        sized(ImageReference::MAX_BYTES),
    ];
    assert_eq!(
        images.iter().map(ImageReference::size).sum::<u64>(),
        UserMessage::MAX_IMAGE_BYTES
    );
    // Ten mebibytes of images are about 13.3 MiB of base64: they fit with a
    // little text, and not with three mebibytes of it, which the text limit
    // of four allows.
    assert_eq!(
        fits_one_frame(
            &message(Some("what is this?"), images.clone()),
            LARGEST_FRAME
        ),
        Ok(())
    );
    let long = "a".repeat(3 * 1024 * 1024);
    assert!(matches!(
        fits_one_frame(&message(Some(&long), images), LARGEST_FRAME),
        Err(AgentError::MessageTooLarge { max_bytes, .. }) if max_bytes == LARGEST_FRAME as u64
    ));
    // Text alone is held to the same frame.
    assert!(matches!(
        fits_one_frame(&message(Some(&long), Vec::new()), 1024 * 1024),
        Err(AgentError::MessageTooLarge { .. })
    ));
}
