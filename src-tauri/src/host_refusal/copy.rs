//! Startup copy shared by typed refusals and the load-failure document.

use serde_json::Value;

fn copies() -> Value {
    serde_json::from_str(include_str!("../../../src/host/startup-refusals.json"))
        .expect("startup refusal sentences are JSON")
}

pub(crate) fn sentence(key: &str) -> String {
    copies()
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_string()
}

pub(crate) fn fill(key: &str, values: &[(&str, &str)]) -> String {
    let mut text = sentence(key);
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// The sentence a person reads. The log sentences stay on [`sentence`].
pub(crate) fn line() -> String {
    sentence("line")
}

/// The copyable token for `key` (`STARTUP_GATEWAY` and the rest).
pub(crate) fn code(key: &str) -> String {
    copies()
        .get("code")
        .and_then(|table| table.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(key)
        .to_string()
}
