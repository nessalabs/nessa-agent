//! Length-prefixed frames on a native connection: a four-byte big-endian
//! length, then that many body bytes. One reader owns the partial prefix and
//! body progress and the announced-length bound, for enrollment envelopes and
//! protected product frames alike; each consumer supplies its own bound.
//!
//! The reader performs no IO. A consumer asks for the bytes the next read
//! should fill, reads into them, and reports how many arrived. An announced
//! length above the bound is refused before any body is allocated, and the
//! refusal is kept: the stream is no longer framed.

/// The announced or encoded length exceeds the consumer's bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTooLarge;

/// Incremental reader of one length-prefixed frame at a time.
/// `frame_reader_refuses_oversize_before_body_and_keeps_partial_progress`
/// exercises the bound, the refusal and partial progress.
pub struct FrameReader {
    limit: usize,
    prefix: [u8; 4],
    prefix_read: usize,
    body: Option<Vec<u8>>,
    body_read: usize,
    refused: bool,
}
impl FrameReader {
    /// A reader admitting bodies of at most `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            prefix: [0; 4],
            prefix_read: 0,
            body: None,
            body_read: 0,
            refused: false,
        }
    }
    /// The bytes the next read should fill: the rest of the prefix, or the
    /// rest of the body. Never empty. Refused once a prefix was too large.
    pub fn unfilled(&mut self) -> Result<&mut [u8], FrameTooLarge> {
        if self.refused {
            return Err(FrameTooLarge);
        }
        Ok(match &mut self.body {
            Some(body) => &mut body[self.body_read..],
            None => &mut self.prefix[self.prefix_read..],
        })
    }
    /// Record `count` bytes read into [`Self::unfilled`]. Returns the body once
    /// it is complete. A complete prefix announcing more than the bound is
    /// refused before its body is allocated.
    pub fn filled(&mut self, count: usize) -> Result<Option<Vec<u8>>, FrameTooLarge> {
        if self.refused {
            return Err(FrameTooLarge);
        }
        if let Some(body) = &self.body {
            self.body_read += count;
            if self.body_read < body.len() {
                return Ok(None);
            }
            self.body_read = 0;
            return Ok(self.body.take());
        }
        self.prefix_read += count;
        if self.prefix_read < self.prefix.len() {
            return Ok(None);
        }
        self.prefix_read = 0;
        let length = u32::from_be_bytes(self.prefix) as usize;
        if length > self.limit {
            self.refused = true;
            return Err(FrameTooLarge);
        }
        if length == 0 {
            return Ok(Some(Vec::new()));
        }
        self.body = Some(vec![0; length]);
        Ok(None)
    }
}

/// Prefix `body` with its length, refusing a body above `limit`.
pub fn encode_frame(limit: usize, body: &[u8]) -> Result<Vec<u8>, FrameTooLarge> {
    if body.len() > limit {
        return Err(FrameTooLarge);
    }
    let length = u32::try_from(body.len()).map_err(|_| FrameTooLarge)?;
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

#[cfg(test)]
#[path = "../../../tests/device_pairing/infrastructure/frames.rs"]
mod tests;
