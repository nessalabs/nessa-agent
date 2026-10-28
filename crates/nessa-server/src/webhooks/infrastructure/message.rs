use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Id;
use serde::Serialize;
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::webhooks::application::{InvalidMessage, Message, MAX_BODY_BYTES};

/// Encode one authorized recordsPage record without changing its keys or
/// escaping. This adapter grants no authority to select a record or checkpoint.
pub fn record_message(
    id: Uuid,
    timestamp_ms: u64,
    conversation: &str,
    incarnation: &str,
    position: u64,
    raw_record: &str,
) -> Result<Message, InvalidMessage> {
    if raw_record.len() > MAX_BODY_BYTES {
        return Err(InvalidMessage);
    }
    ConversationId::new(conversation).map_err(|_| InvalidMessage)?;
    Id::new(incarnation).map_err(|_| InvalidMessage)?;
    // Validate syntax first; the whitespace check then only has to distinguish
    // strings from insignificant whitespace, not reimplement the JSON parser.
    let record: &RawValue = serde_json::from_str(raw_record).map_err(|_| InvalidMessage)?;
    if !compact(raw_record) {
        return Err(InvalidMessage);
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Envelope<'a> {
        conversation_id: &'a str,
        incarnation: &'a str,
        position: u64,
        record: &'a RawValue,
    }
    let body = serde_json::to_vec(&Envelope {
        conversation_id: conversation,
        incarnation,
        position,
        record,
    })
    .map_err(|_| InvalidMessage)?;
    Message::encoded(id, timestamp_ms, body)
}

fn compact(json: &str) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for byte in json.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte.is_ascii_whitespace() {
            return false;
        }
    }
    true
}
