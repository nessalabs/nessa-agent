use std::fmt;
use uuid::Uuid;

/// Stable client-selected UUID. Retrying creation retains this identity.
///
/// The run `-ffff-ffff-ffff-` inside a canonical UUID is not an identity.
/// An observe cursor is the last conversation's id, and the shared-page
/// continuation uses that run, so a conversation that carried it would make
/// the next observe skip the local catalogue.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConversationId(Uuid);
impl ConversationId {
    /// Reject malformed IDs before they can address local storage.
    pub fn new(value: &str) -> Result<Self, &'static str> {
        let id = Uuid::parse_str(value).map_err(|_| "invalid conversation UUID")?;
        if id.to_string() != value {
            return Err("conversation UUID is not canonical lowercase text");
        }
        if value.len() == 36 && value.as_bytes().get(8..23) == Some(b"-ffff-ffff-ffff") {
            return Err("conversation UUID is reserved");
        }
        Ok(Self(id))
    }
}
impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::ConversationId;

    #[test]
    fn a_canonical_uuid_is_a_conversation_and_the_shared_cursor_run_is_not() {
        assert!(ConversationId::new("00000000-0000-4000-8000-000000000001").is_ok());
        assert!(ConversationId::new("ffffffff-0000-4000-8000-000000000000").is_ok());
        assert_eq!(
            ConversationId::new("00000001-ffff-ffff-ffff-000000000000"),
            Err("conversation UUID is reserved")
        );
        assert!(ConversationId::new("00000001-FFFF-FFFF-FFFF-000000000000").is_err());
    }
}
