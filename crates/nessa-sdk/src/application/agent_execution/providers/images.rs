#![deny(missing_docs)]

use crate::domain::{
    agent_execution::prompts::ImageReference, common::value_objects::ImageMediaType,
    model_metadata::value_objects::ImageInputViolation,
};
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Why the bytes an [`ImageReference`] names could not be supplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserImageError {
    /// The source holds no bytes for this digest: never stored, or since removed.
    Missing,
    /// The source could not be read; the same reference may succeed later. An
    /// adapter also answers this for a source that did not finish inside the
    /// adapter's read bound, or that panicked.
    Unavailable,
    /// The supplied bytes disagree with the reference's length or digest. A
    /// source never returns this: the adapter reports it after checking.
    Mismatch,
}
impl fmt::Display for UserImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "the referenced image is not stored"),
            Self::Unavailable => write!(f, "the image store is unavailable"),
            Self::Mismatch => write!(f, "the stored image does not match its reference"),
        }
    }
}
impl Error for UserImageError {}

/// Why a message's images were refused. Nothing was read, saved, queued, or
/// sent: the refusal is about what the message holds and who would receive it,
/// not about the bytes.
///
/// Admission answers this before a message is accepted. An adapter answers the
/// same value at dispatch when it learns only then that its agent takes no
/// images, which can happen after a restoration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageInputRefusal {
    /// The connected agent did not agree to receive images, or this process has
    /// no [`UserImageSource`] to read them from. The selected model may well
    /// accept images; this agent connection does not carry them.
    AgentDoesNotAccept,
    /// The selected model does not accept an image in this encoding.
    MediaType(ImageMediaType),
    /// One image holds more bytes than the selected model accepts.
    ImageTooLarge {
        /// Length of the refused image in bytes.
        size: u64,
        /// Largest image the model accepts, in bytes before base64.
        max_bytes: u64,
    },
}
impl From<ImageInputViolation> for ImageInputRefusal {
    fn from(violation: ImageInputViolation) -> Self {
        match violation {
            ImageInputViolation::MediaType(media_type) => Self::MediaType(media_type),
            ImageInputViolation::TooLarge { size, max_bytes } => {
                Self::ImageTooLarge { size, max_bytes }
            }
        }
    }
}
impl fmt::Display for ImageInputRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentDoesNotAccept => write!(f, "the connected agent does not accept images"),
            Self::MediaType(media_type) => {
                write!(f, "the model does not accept {}", media_type.as_str())
            }
            Self::ImageTooLarge { size, max_bytes } => {
                write!(
                    f,
                    "an image of {size} bytes exceeds the model's {max_bytes}"
                )
            }
        }
    }
}
impl Error for ImageInputRefusal {}

/// Future returned by [`UserImageSource::read`].
pub type UserImageFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, UserImageError>> + Send + 'a>>;

/// Where an adapter gets the bytes a user message refers to.
///
/// A request carries [`ImageReference`] values and never bytes. An adapter that
/// delivers images to its provider resolves each one through this port just
/// before it dispatches, so bytes exist in memory only for that delivery.
/// Composition injects the implementation; a binding given none offers no
/// image input.
///
/// Lookup is by content: the digest names the bytes wherever they came from.
/// Whether a caller may refer to a digest is decided before the request is
/// built, by whoever accepted the reference, not here. The adapter checks the
/// returned length and digest against the reference and refuses a mismatch, so
/// an implementation cannot substitute content.
///
/// An adapter never lets a read hold up anything else. The ACP adapter reads on
/// the calling task, outside the task that owns the agent process, under a
/// bound of its own; closing the context or dropping the call abandons the
/// read by dropping its future, so an implementation must be safe to drop at
/// any await. A read that outlasts the bound, or panics, is
/// [`UserImageError::Unavailable`].
pub trait UserImageSource: Send + Sync {
    /// Return every byte `image` names, and no more.
    ///
    /// The port returns an owned buffer, so it cannot stop an implementation
    /// from building a larger one: an implementation must itself stop at
    /// [`ImageReference::size`] bytes, reading at most one byte past it to
    /// learn that the stored content is longer, and must never return more than
    /// `image.size() + 1` bytes. The adapter compares the length before it
    /// hashes anything and answers [`UserImageError::Mismatch`] for any other
    /// length, so a longer buffer is refused but has already been paid for.
    ///
    /// # Errors
    ///
    /// [`UserImageError::Missing`] when nothing is stored under the digest, and
    /// [`UserImageError::Unavailable`] when the store could not be read.
    fn read(&self, image: ImageReference) -> UserImageFuture<'_>;
}
