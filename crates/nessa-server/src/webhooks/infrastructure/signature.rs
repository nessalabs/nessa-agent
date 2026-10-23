use crate::webhooks::application::{MacKey, Message};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// W-post: three big-endian u64 length-prefixed fields. The timestamp is
/// canonical decimal Unix milliseconds and the body is used byte-for-byte.
pub fn signature(key: &MacKey, message: &Message) -> String {
    let timestamp = message.timestamp_ms().to_string();
    let id = message.id().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(key.expose_bytes()).expect("32-byte HMAC key");
    for field in [timestamp.as_bytes(), id.as_bytes(), message.body()] {
        mac.update(&(field.len() as u64).to_be_bytes());
        mac.update(field);
    }
    let mut result = String::from("v1=");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in mac.finalize().into_bytes() {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}
