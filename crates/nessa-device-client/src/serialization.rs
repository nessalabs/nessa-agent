//! One capped JSON-copy writer for the adapter's saved and product views.
use serde::Serialize;
use std::io::{Error, Result as IoResult, Write};

pub(crate) enum SerializationFailure {
    TooLarge,
    Invalid,
}
struct Bytes {
    value: Vec<u8>,
    maximum: usize,
    exceeded: bool,
}
impl Write for Bytes {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        let length = self.value.len().checked_add(bytes.len());
        let Some(length) = length.filter(|length| *length <= self.maximum) else {
            self.exceeded = true;
            return Err(Error::other("foreign JSON copy bound"));
        };
        if self.value.capacity() < length {
            // Geometric growth stays within the same caller-published cap.
            let capacity = self
                .value
                .capacity()
                .saturating_mul(2)
                .max(length)
                .min(self.maximum);
            self.value.reserve_exact(capacity - self.value.len());
        }
        self.value.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}
pub(crate) fn bounded_json<T: Serialize>(
    value: &T,
    maximum: usize,
) -> Result<String, SerializationFailure> {
    let mut output = Bytes {
        value: Vec::new(),
        maximum,
        exceeded: false,
    };
    if serde_json::to_writer(&mut output, value).is_err() {
        return Err(if output.exceeded {
            SerializationFailure::TooLarge
        } else {
            SerializationFailure::Invalid
        });
    }
    String::from_utf8(output.value).map_err(|_| SerializationFailure::Invalid)
}
#[cfg(test)]
#[path = "../tests/product/serialization.rs"]
mod tests;
