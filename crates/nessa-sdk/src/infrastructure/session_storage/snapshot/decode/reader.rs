//! Limit tokens before Serde's reader-backed scratch buffer can grow.
use std::{
    cell::Cell,
    io::{self, Read},
    rc::Rc,
};

pub(super) struct TokenReader<R> {
    source: R,
    limit: Rc<Cell<usize>>,
    string: bool,
    escaped: bool,
    bytes: usize,
    number: usize,
}
impl<R> TokenReader<R> {
    pub(super) fn new(source: R, limit: Rc<Cell<usize>>) -> Self {
        Self {
            source,
            limit,
            string: false,
            escaped: false,
            bytes: 0,
            number: 0,
        }
    }
}
impl<R: Read> Read for TokenReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        // Serde requests individual bytes. Restrict reads anyway so changing the
        // selected field limit never applies retroactively to prefetched tokens.
        if out.is_empty() {
            return Ok(0);
        }
        let count = self.source.read(&mut out[..1])?;
        if count == 0 {
            return Ok(0);
        }
        let byte = out[0];
        if self.string {
            if byte == b'"' && !self.escaped {
                self.string = false;
            } else {
                self.bytes += 1;
                // Any valid JSON encoding uses at most six source bytes per
                // decoded UTF-8 byte, including surrogate pairs. The visitor
                // separately enforces the exact decoded limit.
                if self.bytes > self.limit.get().saturating_mul(6) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "journal string exceeds decoding limit",
                    ));
                }
                self.escaped = byte == b'\\' && !self.escaped;
            }
        } else if byte == b'"' {
            self.string = true;
            self.escaped = false;
            self.bytes = 0;
            self.number = 0;
        } else if byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E') {
            self.number += 1;
            if self.number > 128 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal numeric token exceeds decoding limit",
                ));
            }
        } else {
            self.number = 0;
        }
        Ok(count)
    }
}
