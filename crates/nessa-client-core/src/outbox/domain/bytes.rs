//! Canonical bytes for one phone send. One encoding, so a retry cannot drift.

use super::intent::{IntentError, PhoneCommandId, PhoneImage, PhoneIntent};
use nessa_sdk::domain::agent_execution::executions::ExecutionId;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};

const MARK: &[u8] = b"NESSA_PHONE_SEND_1";

pub(super) fn encode(intent: &PhoneIntent) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MARK.len() + intent.text().len() + 64);
    bytes.extend_from_slice(MARK);
    push_str(&mut bytes, &intent.conversation().to_string());
    push_str(&mut bytes, intent.command().as_str());
    push_str(&mut bytes, intent.turn().as_str());
    push_str(&mut bytes, intent.text());
    let count = u16::try_from(intent.images().len()).expect("image count fits in a u16");
    bytes.extend_from_slice(&count.to_be_bytes());
    for image in intent.images() {
        push_str(&mut bytes, &image.digest());
        push_str(&mut bytes, image.media_type());
        bytes.extend_from_slice(&image.size().to_be_bytes());
    }
    bytes
}

pub(super) fn decode(bytes: &[u8]) -> Result<PhoneIntent, IntentError> {
    let mut cursor = Cursor { bytes };
    let mark = cursor.take(MARK.len()).map_err(|_| IntentError::Bytes)?;
    if mark != MARK {
        return Err(IntentError::Bytes);
    }
    let conversation = cursor.string()?;
    let command = PhoneCommandId::new(cursor.string()?).map_err(|_| IntentError::Bytes)?;
    let turn = ExecutionId::new(cursor.string()?).map_err(|_| IntentError::Bytes)?;
    let text = cursor.string()?;
    let count = cursor.u16()?;
    let mut images = Vec::with_capacity(count);
    for _ in 0..count {
        let digest = Sha256Digest::parse(&cursor.string()?).map_err(|_| IntentError::Bytes)?;
        let media = ImageMediaType::parse(&cursor.string()?).map_err(|_| IntentError::Bytes)?;
        let size = cursor.u64()?;
        images.push(PhoneImage::new(digest, media, size).map_err(|_| IntentError::Bytes)?);
    }
    if !cursor.bytes.is_empty() {
        return Err(IntentError::Bytes);
    }
    let intent = PhoneIntent::new(&conversation, command, turn, text, images)
        .map_err(|_| IntentError::Bytes)?;
    if intent.canonical() != bytes {
        return Err(IntentError::Bytes);
    }
    Ok(intent)
}

fn push_str(bytes: &mut Vec<u8>, value: &str) {
    // Command ids, turn ids, conversation ids, text, digests, and media types
    // are bounded under u16::MAX before encode is called.
    let len = u16::try_from(value.len()).expect("validated phone field fits in a u16 length");
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
}
impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], ()> {
        if self.bytes.len() < len {
            return Err(());
        }
        let (head, tail) = self.bytes.split_at(len);
        self.bytes = tail;
        Ok(head)
    }
    fn u16(&mut self) -> Result<usize, IntentError> {
        let raw: [u8; 2] = self
            .take(2)
            .map_err(|_| IntentError::Bytes)?
            .try_into()
            .map_err(|_| IntentError::Bytes)?;
        Ok(u16::from_be_bytes(raw) as usize)
    }
    fn u64(&mut self) -> Result<u64, IntentError> {
        let raw: [u8; 8] = self
            .take(8)
            .map_err(|_| IntentError::Bytes)?
            .try_into()
            .map_err(|_| IntentError::Bytes)?;
        Ok(u64::from_be_bytes(raw))
    }
    fn string(&mut self) -> Result<String, IntentError> {
        let len = self.u16()?;
        let bytes = self.take(len).map_err(|_| IntentError::Bytes)?;
        let text = std::str::from_utf8(bytes).map_err(|_| IntentError::Bytes)?;
        Ok(text.to_owned())
    }
}
