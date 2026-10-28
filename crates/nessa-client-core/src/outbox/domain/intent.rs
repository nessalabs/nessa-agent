//! One phone send: the conversation, the command id, the turn id, and the text.

use super::bytes::{decode, encode};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product_contract::generated::MAX_MCP_MESSAGE_BYTES;
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::agent_execution::prompts::{ImageReference, PromptText, UserMessage};
use nessa_sdk::domain::agent_execution::ExecutionError;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use std::fmt;

/// Why a typed send was not an intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentError {
    /// The command id or the turn id is blank.
    EmptyId,
    /// The command id, the turn id, or the text is past its bound.
    TooLong,
    /// There is no text and no image.
    Empty,
    /// More images than one message may name, or more image bytes.
    Images,
    /// An image digest, media type, or size was refused.
    Image,
    /// The conversation id is not a canonical lowercase UUID.
    Conversation,
    /// Stored canonical bytes are not this version.
    Bytes,
}

/// The command id. One per principal, shared with `conversation.receipt`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PhoneCommandId(ExecutionId);
impl PhoneCommandId {
    /// Keep `value` exactly. Blank and over [`ExecutionId::MAX_BYTES`] are refused.
    pub fn new(value: impl Into<String>) -> Result<Self, IntentError> {
        match ExecutionId::new(value) {
            Ok(id) => Ok(Self(id)),
            Err(ExecutionError::ValueTooLong { .. }) => Err(IntentError::TooLong),
            Err(_) => Err(IntentError::EmptyId),
        }
    }
    /// The id as `conversation.send` will carry it.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
impl fmt::Display for PhoneCommandId {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(self.as_str())
    }
}

/// One image already uploaded. The bytes themselves stay on the attachment path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhoneImage {
    reference: ImageReference,
}
impl PhoneImage {
    /// Refer to `size` bytes of `media` hashing to `digest`.
    pub fn new(
        digest: Sha256Digest,
        media: ImageMediaType,
        size: u64,
    ) -> Result<Self, IntentError> {
        ImageReference::new(digest, media, size)
            .map(|reference| Self { reference })
            .map_err(|_| IntentError::Image)
    }
    /// Digest text, `sha256:` and 64 lowercase hex digits.
    pub fn digest(&self) -> String {
        self.reference.digest().to_string()
    }
    /// `image/png`, `image/jpeg`, `image/gif`, or `image/webp`.
    pub fn media_type(&self) -> &'static str {
        self.reference.media_type().as_str()
    }
    /// Length in bytes.
    pub fn size(&self) -> u64 {
        self.reference.size()
    }
}

/// The canonical send. Its bytes are what a retry and a lookup repeat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhoneIntent {
    conversation: ConversationId,
    command: PhoneCommandId,
    turn: ExecutionId,
    text: String,
    images: Box<[PhoneImage]>,
    canonical: Box<[u8]>,
}
impl PhoneIntent {
    /// Validate and freeze the send.
    ///
    /// Text may be empty only when `images` is not. The text bound is
    /// [`MAX_MCP_MESSAGE_BYTES`], the published ceiling `conversation.send`
    /// is held to. Files are empty, as the phone send contract requires.
    pub fn new(
        conversation: &str,
        command: PhoneCommandId,
        turn: ExecutionId,
        text: impl Into<String>,
        images: Vec<PhoneImage>,
    ) -> Result<Self, IntentError> {
        let conversation =
            ConversationId::new(conversation).map_err(|_| IntentError::Conversation)?;
        let text = text.into();
        if text.len() > MAX_MCP_MESSAGE_BYTES {
            return Err(IntentError::TooLong);
        }
        let prompt = if text.trim().is_empty() {
            None
        } else {
            Some(PromptText::new(text.clone()).map_err(|_| IntentError::Empty)?)
        };
        let references = images.iter().map(|image| image.reference).collect();
        UserMessage::new(prompt, references, Vec::new(), Vec::new()).map_err(
            |error| match error {
                ExecutionError::TooManyValues { .. } | ExecutionError::ValueTooLong { .. } => {
                    IntentError::Images
                }
                _ => IntentError::Empty,
            },
        )?;
        let intent = Self {
            conversation,
            command,
            turn,
            text,
            images: images.into_boxed_slice(),
            canonical: Box::new([]),
        };
        let canonical = encode(&intent).into_boxed_slice();
        Ok(Self {
            canonical,
            ..intent
        })
    }
    /// Rebuild an intent from bytes this module wrote.
    pub(crate) fn from_canonical(bytes: &[u8]) -> Result<Self, IntentError> {
        decode(bytes)
    }
    /// The conversation the send names.
    pub fn conversation(&self) -> &ConversationId {
        &self.conversation
    }
    /// The command id.
    pub fn command(&self) -> &PhoneCommandId {
        &self.command
    }
    /// The turn id `conversation.send` calls `executionId`.
    pub fn turn(&self) -> &ExecutionId {
        &self.turn
    }
    /// The text the person typed, including whitespace.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Uploaded images, in the order they were attached.
    pub fn images(&self) -> &[PhoneImage] {
        &self.images
    }
    /// Bytes a retry and a lookup must repeat.
    pub fn canonical(&self) -> &[u8] {
        &self.canonical
    }
    /// Whether `other` is this send, byte for byte.
    pub fn same_bytes(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
    /// The same text and images under new ids. Used when `turn_running`
    /// returns the prompt to a draft (`turn_running_returns_the_text_to_a_new_draft`).
    pub(crate) fn again(
        &self,
        command: PhoneCommandId,
        turn: ExecutionId,
    ) -> Result<Self, IntentError> {
        Self::new(
            &self.conversation.to_string(),
            command,
            turn,
            self.text.clone(),
            self.images.clone().into_vec(),
        )
    }
}
