#![deny(missing_docs)]

use super::PromptText;
use crate::domain::{
    agent_execution::ExecutionError,
    common::value_objects::{ImageMediaType, Sha256Digest},
};

/// One image a user message refers to: what the bytes hash to, how they are
/// encoded, and how many there are. It never holds the bytes, so a message
/// stays small enough to compare, queue, and persist whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageReference {
    digest: Sha256Digest,
    media_type: ImageMediaType,
    size: u64,
}
impl ImageReference {
    /// Largest single image, in bytes.
    pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

    /// Refer to `size` bytes of `media_type` hashing to `digest`. An empty image
    /// is [`ExecutionError::EmptyValue`]; one over [`Self::MAX_BYTES`] is
    /// [`ExecutionError::ValueTooLong`]. Nothing is read: whoever resolves the
    /// reference verifies that the bytes agree with it.
    pub fn new(
        digest: Sha256Digest,
        media_type: ImageMediaType,
        size: u64,
    ) -> Result<Self, ExecutionError> {
        if size == 0 {
            return Err(ExecutionError::EmptyValue("image"));
        }
        if size > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "image",
                max_bytes: Self::MAX_BYTES as usize,
            });
        }
        Ok(Self {
            digest,
            media_type,
            size,
        })
    }
    /// Digest of the image bytes.
    pub fn digest(&self) -> Sha256Digest {
        self.digest
    }
    /// Encoding of the image bytes.
    pub fn media_type(&self) -> ImageMediaType {
        self.media_type
    }
    /// Length of the image in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// One file a user message points the agent at by name, never by content.
///
/// Nessa runs the agent on the machine the message was written on, so a file
/// that is not an image does not travel: the message carries where it is and
/// the agent opens it itself, if it decides to. Nothing here is read — the path
/// is not resolved, followed, or checked for existence, because whether the
/// file is there is only true or false at the moment the agent opens it, which
/// is later than this and after a person has approved that read.
///
/// The invariants are about what a path *means*, and they are the value's own because the alternative is an adapter quietly mangling a path
/// into one that names a different file:
///
/// - It is absolute. A relative path means nothing without the directory it is
///   relative to, and the agent's is not the panel's.
/// - It holds no control character. No path needs one, and a path with one
///   cannot be shown to whoever approves the read, written into an audit
///   record, or read back out of a log as the same path.
/// - It ends in a file name, so there is something to call it. A path ending in
///   `/` names a directory.
/// - Every component below the root is a name: not empty, and not `.` or `..`.
///   None of those survives being written as a URI — an empty component is
///   dropped and a dot component is resolved away — so the link would name a
///   path that is not the one given. A path beginning `//` is also
///   implementation-defined in POSIX, so that difference can be a different
///   file. No picker produces any of them.
///
/// Nothing here is a rule about markdown, and that is deliberate. This value
/// used to refuse `[` and `]` anywhere in a path, because the ACP adapter
/// writes the file into the prompt as `[@name](uri)` and a bracket could close
/// the label. Twice that reasoning turned out to cover only the characters
/// whoever wrote it had thought of — first the `)` that ends a destination,
/// then the `\` that escapes whatever follows it. The rule is gone rather than
/// extended a third time: the adapter now percent-encodes the URI down to an
/// allowlist and backslash-escapes every ASCII punctuation character in the
/// label, so no path can become syntax there whatever the grammar turns out to
/// say — and `[draft] notes.pdf` is an ordinary name again. Representation
/// belongs to whoever is doing the representing.
///
/// Whitespace is not trimmed: a file may legitimately be named with it, and
/// trimming would produce a path that names something else. A path that is not
/// UTF-8 cannot be described here at all, and whoever obtained it says so
/// rather than passing on a lossy rendering.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LinkedFile {
    path: Box<str>,
}
impl LinkedFile {
    /// Longest path, in bytes. `PATH_MAX` on the systems this runs on, so a
    /// longer value names nothing that could be opened anyway.
    pub const MAX_PATH_BYTES: usize = 4096;

    /// Point at the file at `path`.
    ///
    /// # Errors
    ///
    /// - [`ExecutionError::EmptyValue`] for an empty path.
    /// - [`ExecutionError::ValueTooLong`] over [`Self::MAX_PATH_BYTES`].
    /// - [`ExecutionError::RelativeFilePath`] when it does not start at the
    ///   root.
    /// - [`ExecutionError::ControlCharacterInFilePath`] for a control
    ///   character anywhere in it.
    /// - [`ExecutionError::FilePathWithoutName`] when it ends in a separator.
    /// - [`ExecutionError::RepeatedSeparatorInFilePath`] for a doubled
    ///   separator anywhere in it.
    /// - [`ExecutionError::UnresolvedFilePathComponent`] for a `.` or `..`
    ///   component anywhere in it.
    ///
    /// # Examples
    ///
    /// ```
    /// use nessa_sdk::domain::agent_execution::prompts::LinkedFile;
    ///
    /// let file = LinkedFile::new("/Users/ada/notes/report.pdf".into())?;
    /// assert_eq!(file.name(), "report.pdf");
    /// assert!(LinkedFile::new("notes/report.pdf".into()).is_err());
    /// # Ok::<(), nessa_sdk::domain::agent_execution::ExecutionError>(())
    /// ```
    pub fn new(path: String) -> Result<Self, ExecutionError> {
        if path.is_empty() {
            return Err(ExecutionError::EmptyValue("linked file path"));
        }
        if path.len() > Self::MAX_PATH_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "linked file path",
                max_bytes: Self::MAX_PATH_BYTES,
            });
        }
        if !path.starts_with('/') {
            return Err(ExecutionError::RelativeFilePath);
        }
        if path.chars().any(char::is_control) {
            return Err(ExecutionError::ControlCharacterInFilePath);
        }
        if path.ends_with('/') {
            return Err(ExecutionError::FilePathWithoutName);
        }
        // Every component below the root has to be a name, because a component
        // that is not one does not survive being written as a URI: an empty
        // component is dropped and a `.` or `..` is resolved away, either of
        // which leaves the link naming a path that is not the one given. That
        // is the one thing this value exists to prevent, and no file picker
        // produces such a path, so refusing is both honest and free.
        for component in path[1..].split('/') {
            if component.is_empty() {
                return Err(ExecutionError::RepeatedSeparatorInFilePath);
            }
            if component == "." || component == ".." {
                return Err(ExecutionError::UnresolvedFilePathComponent);
            }
        }
        Ok(Self { path: path.into() })
    }
    /// The whole path, exactly as it was given.
    pub fn path(&self) -> &str {
        &self.path
    }
    /// What to call the file: the last component of its path, which the
    /// constructor established is there. This is a label, not an identity —
    /// two linked files can share a name and be different files.
    pub fn name(&self) -> &str {
        Self::file_name(&self.path).unwrap_or_default()
    }
    /// The last component of `path`, or `None` when it ends in a separator and
    /// so names a directory.
    fn file_name(path: &str) -> Option<&str> {
        match path.rsplit('/').next() {
            Some("") | None => None,
            Some(name) => Some(name),
        }
    }
}

/// What a user said in one turn: text, the images it carries, and the files it
/// points at, in that order.
///
/// Text is optional because an image or a file alone is a complete message; a
/// message with none of the three is not one. Images and files each keep the
/// order the user attached them in.
///
/// Images and files are separate because they are carried in opposite ways.
/// An image's bytes travel with the message and are budgeted against the frame
/// that has to hold them; a file's do not travel at all, so no byte budget
/// applies and the only bound is how many paths one message may name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMessage {
    text: Option<PromptText>,
    images: Box<[ImageReference]>,
    files: Box<[LinkedFile]>,
}
impl UserMessage {
    /// Most images in one message.
    pub const MAX_IMAGES: usize = 10;
    /// Most image bytes in one message, across all of its images.
    ///
    /// Encoded as base64 for a provider they grow by a third, to about
    /// 13.4 MiB, which leaves room for some text inside the 16 MiB that is the
    /// largest frame an adapter carries. It does not leave room for all the
    /// text a message may hold: the most images together with a few mebibytes
    /// of text is a valid message that no single frame fits. That is a rule of
    /// the adapter and its configured frame, not of a message, so the adapter
    /// refuses such a message by type when it is submitted, before it is
    /// accepted, rather than this constructor refusing it.
    pub const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

    /// Most files one message may point at.
    ///
    /// The same figure as [`Self::MAX_IMAGES`], for the same reason: it is how
    /// many attachments a person plausibly means in one turn, not a size. No
    /// byte budget stands beside it, because the paths are all that travel —
    /// ten of them at [`LinkedFile::MAX_PATH_BYTES`] is under 40 KiB, far
    /// inside any frame an adapter carries.
    pub const MAX_FILES: usize = 10;

    /// Combine optional `text` with `images` and `files`, each in attachment
    /// order. All three empty is [`ExecutionError::EmptyValue`]; more than
    /// [`Self::MAX_IMAGES`] or [`Self::MAX_FILES`] is
    /// [`ExecutionError::TooManyValues`]; more than [`Self::MAX_IMAGE_BYTES`]
    /// in total is [`ExecutionError::ValueTooLong`]. The same image or file may
    /// appear twice; each appearance counts toward its budget.
    pub fn new(
        text: Option<PromptText>,
        images: Vec<ImageReference>,
        files: Vec<LinkedFile>,
    ) -> Result<Self, ExecutionError> {
        if text.is_none() && images.is_empty() && files.is_empty() {
            return Err(ExecutionError::EmptyValue("user message"));
        }
        if files.len() > Self::MAX_FILES {
            return Err(ExecutionError::TooManyValues {
                field: "user message files",
                max: Self::MAX_FILES,
            });
        }
        if images.len() > Self::MAX_IMAGES {
            return Err(ExecutionError::TooManyValues {
                field: "user message images",
                max: Self::MAX_IMAGES,
            });
        }
        // Each size is at most `ImageReference::MAX_BYTES`, so ten cannot overflow.
        if images.iter().map(ImageReference::size).sum::<u64>() > Self::MAX_IMAGE_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "user message images",
                max_bytes: Self::MAX_IMAGE_BYTES as usize,
            });
        }
        Ok(Self {
            text,
            images: images.into_boxed_slice(),
            files: files.into_boxed_slice(),
        })
    }
    /// A message of text alone, which cannot fail: the text is already nonblank.
    pub fn text_only(text: PromptText) -> Self {
        Self {
            text: Some(text),
            images: Box::default(),
            files: Box::default(),
        }
    }
    /// The text, when the user wrote any.
    pub fn text(&self) -> Option<&PromptText> {
        self.text.as_ref()
    }
    /// The exact text, or the empty string for a message of images alone.
    pub fn text_str(&self) -> &str {
        self.text.as_ref().map_or("", PromptText::as_str)
    }
    /// Images in attachment order; empty for a message of text alone.
    pub fn images(&self) -> &[ImageReference] {
        &self.images
    }
    /// Files this message points at, in attachment order; empty when it points
    /// at none. Nothing here has been opened.
    pub fn files(&self) -> &[LinkedFile] {
        &self.files
    }
}
