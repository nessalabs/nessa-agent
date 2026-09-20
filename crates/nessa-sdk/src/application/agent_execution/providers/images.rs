#![deny(missing_docs)]

use crate::domain::agent_execution::prompts::ImageReference;
use std::{error::Error, fmt, future::Future, pin::Pin};

/// Why the bytes an [`ImageReference`] names could not be supplied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserImageError {
    /// The source holds no bytes for this digest: never stored, or since removed.
    Missing,
    /// The source could not be read; the same reference may succeed later.
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

/// Future returned by [`UserImageSource::read`].
pub type UserImageFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, UserImageError>> + Send + 'a>>;

/// Where an adapter gets the bytes a user message refers to.
///
/// A request carries [`ImageReference`] values and never bytes. An adapter that
/// delivers images to its provider resolves each one through this port when it
/// dispatches, so bytes exist in memory only for that write. Composition
/// injects the implementation; a binding given none offers no image input.
///
/// Lookup is by content: the digest names the bytes wherever they came from.
/// Whether a caller may refer to a digest is decided before the request is
/// built, by whoever accepted the reference, not here. The adapter checks the
/// returned length and digest against the reference and refuses a mismatch, so
/// an implementation cannot substitute content.
pub trait UserImageSource: Send + Sync {
    /// Return every byte `image` names. At most [`ImageReference::MAX_BYTES`]
    /// are expected; the caller rejects any other length.
    fn read(&self, image: ImageReference) -> UserImageFuture<'_>;
}
