use serde_json::Value;
use std::io::{self, Write};

/// Whether `value`'s JSON text is at most `bytes` long, counted as it is
/// written and abandoned at the bound, so a value of any size costs no more
/// than the bound to measure.
pub(crate) fn json_fits(value: &Value, bytes: usize) -> bool {
    struct Budget(usize);
    impl Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or_else(|| io::Error::other("past the bound"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Budget(bytes), value).is_ok()
}
