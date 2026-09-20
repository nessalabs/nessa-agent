use std::{error::Error, fmt};

/// Why a value could not describe an attachment, its caller, or its ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentError {
    /// Not a lowercase `type/subtype` of 3 to 127 bytes without parameters.
    MediaType,
    /// Empty, or over [`super::Attachment::MAX_BYTES`].
    Size,
    /// A surface or action identifier that is blank, oversized, or holds
    /// control characters.
    Caller,
    /// A deadline that cannot be represented.
    Lifetime,
}
impl fmt::Display for AttachmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MediaType => write!(f, "media type must be a lowercase type/subtype"),
            Self::Size => write!(f, "attachment size must be between 1 byte and 20 MiB"),
            Self::Caller => write!(f, "caller surface and action must be bounded plain text"),
            Self::Lifetime => write!(f, "ticket deadline cannot be represented"),
        }
    }
}
impl Error for AttachmentError {}
