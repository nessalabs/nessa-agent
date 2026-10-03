use crate::application::agent_execution::agents::AgentError;
use crate::infrastructure::json_rpc::protocol;
use serde_json::Value;
use std::io::{self, Write};

/// The longest provider identifier (a session's, a tool call's) kept.
pub(crate) const MAX_IDENTIFIER_BYTES: usize = 256;

pub(crate) fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| protocol(&format!("missing {field}")))
}
pub(crate) fn identifier<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentError> {
    let value = string(value, field)?;
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(protocol("identifier exceeds limit"));
    }
    Ok(value)
}

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
