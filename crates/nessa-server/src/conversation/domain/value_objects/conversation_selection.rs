//! Immutable choices fixed or committed on one conversation.

/// A catalog model ID. Availability belongs to gateway composition; this
/// value only prevents an invalid identity reaching durable storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationModelId(String);

impl ConversationModelId {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err("invalid conversation model id");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Product choice whose native meaning is owned by the selected ACP binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationApprovalMode {
    Ask,
    Auto,
    Full,
}

impl ConversationApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}
