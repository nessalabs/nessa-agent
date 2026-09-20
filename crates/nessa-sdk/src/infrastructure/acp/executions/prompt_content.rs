//! A user message as ACP content blocks: how large they will be, where the
//! bytes behind the images come from, and the blocks themselves.
//!
//! ```text
//! admission ──► fits_one_frame            (sizes only; nothing is read)
//! session   ──► read_images ──► ImageBlocks ──► worker ──► content_blocks
//!                  │                 │
//!                  │                 └── the session's in-flight byte budget,
//!                  │                     taken before the read, given back
//!                  │                     once these bytes are a frame
//!                  └── UserImageSource, bounded, abandoned on close
//! ```
//!
//! Arrows are calls in the order they happen. The read runs on the calling
//! task, never on the worker: the worker is the only task that polls close,
//! deadlines, consumer loss, and the agent's output, so nothing it awaits may
//! depend on an injected source. The worker receives blocks that are already
//! verified and encoded.
//!
//! Encoded image bytes travel in the command queue, so a session bounds how
//! many of them may be waiting there at once; see [`encoded_image_bytes`] and
//! `AcpConfig::images`.
use crate::application::agent_execution::{
    agents::AgentError,
    providers::{ImageInputRefusal, UserImageError, UserImageSource},
};
use crate::domain::agent_execution::prompts::{ImageReference, UserMessage};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    future::{poll_fn, Future},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::pin,
    task::Poll,
    time::Duration,
};
use tokio::sync::OwnedSemaphorePermit;

/// Longest wait for every image of one message together.
///
/// A source reads local storage, where ten mebibytes take milliseconds, so ten
/// seconds is already a failure rather than a slow success. The bound is fixed
/// rather than configured because no caller has a reason to wait longer, and it
/// applies whether or not the execution itself has a timeout: a read is not
/// part of the agent's run and must never be unbounded because the run is.
/// A shorter execution or steering deadline shortens it further.
pub(in crate::infrastructure::acp) const IMAGE_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Bytes allowed for everything in a request that is not the message: the
/// JSON-RPC envelope, the method, the session identifier (at most 256 bytes,
/// each of which JSON may escape to six), and steering metadata.
const REQUEST_ALLOWANCE_BYTES: u64 = 2048;
/// `{"type":"text","text":}` and the comma after it.
const TEXT_BLOCK_BYTES: u64 = 24;
/// `{"type":"image","mimeType":"","data":""}`, the comma after it, and the
/// media type, generously.
const IMAGE_BLOCK_BYTES: u64 = 64;

/// Image blocks ready to send, in the message's attachment order. Each was
/// read, checked against its reference, and encoded before this existed.
///
/// The value also carries this message's share of its session's in-flight
/// image budget. The share is taken before the first byte is read and is given
/// back when the blocks have become a frame, or as soon as they are dropped.
pub(crate) struct ImageBlocks {
    blocks: Vec<Value>,
    charge: Option<OwnedSemaphorePermit>,
}
impl ImageBlocks {
    /// The blocks of a message that refers to no image. Nothing is charged for
    /// a message that carries no bytes.
    pub(crate) fn none() -> Self {
        Self {
            blocks: Vec::new(),
            charge: None,
        }
    }
    /// The same blocks, holding `charge` until it is taken or dropped.
    pub(in crate::infrastructure::acp) fn charged(
        mut self,
        charge: Option<OwnedSemaphorePermit>,
    ) -> Self {
        self.charge = charge;
        self
    }
    /// Hand the budget share to the caller, which holds it until these bytes
    /// have been written as a frame.
    pub(in crate::infrastructure::acp) fn take_charge(&mut self) -> Option<OwnedSemaphorePermit> {
        self.charge.take()
    }
}

/// How many bytes `message`'s images occupy once base64 has grown them by a
/// third: what one message costs the session's in-flight image budget.
///
/// A message admitted by [`fits_one_frame`] always fits one frame, so this is
/// never larger than the configured frame size.
pub(in crate::infrastructure::acp) fn encoded_image_bytes(message: &UserMessage) -> u32 {
    let bytes: u64 = message
        .images()
        .iter()
        .map(|image| image.size().div_ceil(3) * 4)
        .sum();
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

/// Refuse a message that cannot fit one frame of `max_frame_bytes` once its
/// text is JSON and its images are base64. Only lengths are used; nothing is
/// read or encoded. The figure is the exact size of the content blocks plus a
/// fixed allowance for the request around them, so a message within about two
/// kibibytes of the limit may be refused although it would have fit; one that
/// passes always fits.
pub(in crate::infrastructure::acp) fn fits_one_frame(
    message: &UserMessage,
    max_frame_bytes: usize,
) -> Result<(), AgentError> {
    let text = message.text().map_or(0, |text| {
        TEXT_BLOCK_BYTES + json_string_bytes(text.as_str())
    });
    let images: u64 = message
        .images()
        .iter()
        .map(|image| IMAGE_BLOCK_BYTES + image.size().div_ceil(3) * 4)
        .sum();
    let encoded_bytes = REQUEST_ALLOWANCE_BYTES + text + images;
    let max_bytes = max_frame_bytes as u64;
    if encoded_bytes > max_bytes {
        return Err(AgentError::MessageTooLarge {
            encoded_bytes,
            max_bytes,
        });
    }
    Ok(())
}

/// Length of `text` as a JSON string, quotes included, as `serde_json` writes
/// it: `"` and `\` and the five short escapes take two bytes, every other
/// control character six, and everything else is copied.
fn json_string_bytes(text: &str) -> u64 {
    2 + text
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | 0x08 | 0x09 | 0x0a | 0x0c | 0x0d => 2,
            0x00..=0x1f => 6,
            _ => 1,
        })
        .sum::<u64>()
}

/// Read, verify, and encode every image `message` refers to.
///
/// `limit` bounds the whole read and `stopped` abandons it: the caller passes
/// a future that resolves when its context is closing or its worker is gone.
/// Both drop the source's future where it stands. A message without images
/// answers at once and touches neither.
///
/// # Errors
///
/// - [`ImageInputRefusal::NotOffered`] without a `source`: this process has
///   nowhere to read bytes from, so the binding carries no image at all.
/// - [`AgentError::Closed`] when `stopped` resolves first.
/// - [`UserImageError::Unavailable`] when `limit` passes first or the source
///   panics; the source's own error otherwise.
/// - [`UserImageError::Mismatch`] when the bytes are not the referenced ones.
pub(in crate::infrastructure::acp) async fn read_images(
    source: Option<&dyn UserImageSource>,
    message: &UserMessage,
    limit: Duration,
    stopped: impl Future<Output = ()>,
) -> Result<ImageBlocks, AgentError> {
    if message.images().is_empty() {
        return Ok(ImageBlocks::none());
    }
    let source = source.ok_or(AgentError::ImageInputRefused(ImageInputRefusal::NotOffered))?;
    tokio::select! { biased;
        () = stopped => Err(AgentError::Closed),
        () = tokio::time::sleep(limit) => Err(AgentError::UserImage(UserImageError::Unavailable)),
        blocks = read_all(source, message.images()) => blocks.map_err(AgentError::UserImage),
    }
}

async fn read_all(
    source: &dyn UserImageSource,
    images: &[ImageReference],
) -> Result<ImageBlocks, UserImageError> {
    let mut blocks = Vec::with_capacity(images.len());
    for image in images {
        let bytes = read_one(source, *image).await?;
        // Length first: it is free, and it keeps an oversized answer from
        // being hashed at all.
        if bytes.len() as u64 != image.size() {
            return Err(UserImageError::Mismatch);
        }
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        if &digest != image.digest().as_bytes() {
            return Err(UserImageError::Mismatch);
        }
        // Encode, then drop the raw bytes before the block is built, so one
        // image is held both ways only while it is being encoded.
        let encoded = STANDARD.encode(&bytes);
        drop(bytes);
        blocks.push(json!({
            "type": "image",
            "mimeType": image.media_type().as_str(),
            "data": encoded,
        }));
    }
    Ok(ImageBlocks {
        blocks,
        charge: None,
    })
}

/// One read, with a panic in the injected source turned into a typed failure
/// instead of unwinding through the caller's task.
async fn read_one(
    source: &dyn UserImageSource,
    image: ImageReference,
) -> Result<Vec<u8>, UserImageError> {
    let Ok(read) = catch_unwind(AssertUnwindSafe(|| source.read(image))) else {
        tracing::error!("user image source panicked while starting a read");
        return Err(UserImageError::Unavailable);
    };
    let mut read = pin!(read);
    poll_fn(
        |context| match catch_unwind(AssertUnwindSafe(|| read.as_mut().poll(context))) {
            Ok(poll) => poll,
            Err(_) => {
                tracing::error!("user image source panicked during a read");
                Poll::Ready(Err(UserImageError::Unavailable))
            }
        },
    )
    .await
}

/// The content blocks for `message`: its text, then its images in attachment
/// order. `images` must be what [`read_images`] answered for this message.
///
/// # Errors
///
/// [`AgentError::Protocol`] when the block count disagrees with the message,
/// which no caller in this crate can cause; nothing is sent in that case.
pub(in crate::infrastructure::acp) fn content_blocks(
    message: &UserMessage,
    images: ImageBlocks,
) -> Result<Vec<Value>, AgentError> {
    if images.blocks.len() != message.images().len() {
        return Err(AgentError::Protocol(
            "resolved image blocks do not match the message".into(),
        ));
    }
    let mut blocks = Vec::with_capacity(1 + images.blocks.len());
    if let Some(text) = message.text() {
        blocks.push(json!({"type":"text","text":text.as_str()}));
    }
    blocks.extend(images.blocks);
    Ok(blocks)
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/executions/prompt_content.rs"]
mod tests;
