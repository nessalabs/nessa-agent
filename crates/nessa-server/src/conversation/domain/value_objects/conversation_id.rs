use std::fmt;
use uuid::Uuid;

/// Stable client-selected UUID. Retrying creation retains this identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConversationId(Uuid);
impl ConversationId {
    /// Reject malformed IDs before they can address local storage.
    pub fn new(value: &str) -> Result<Self, &'static str> {
        let id = Uuid::parse_str(value).map_err(|_| "invalid conversation UUID")?;
        if id.to_string() != value {
            return Err("conversation UUID is not canonical lowercase text");
        }
        Ok(Self(id))
    }
}
impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
