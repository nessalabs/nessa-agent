//! Token and context figures an ACP payload actually carried.
//!
//! `usage_update` reports how full the session context is (`used`, `size`),
//! not model input and output. Input and output tokens are recorded only from
//! fields a prompt result or update actually has.

use crate::application::agent_execution::traces::ReportedUsage;
use serde_json::Value;

/// Figures present on `value`. Absent fields stay absent.
pub(crate) fn reported_usage(value: &Value) -> ReportedUsage {
    let mut usage = from_object(value);
    if let Some(nested) = value.get("usage").filter(|nested| nested.is_object()) {
        usage = merge(usage, from_tokens(nested));
    }
    usage
}

fn from_object(value: &Value) -> ReportedUsage {
    let mut usage = from_tokens(value);
    if let Some(tokens) = number(value, &["used"]) {
        usage = usage.with_context_used(tokens);
    }
    if let Some(tokens) = number(value, &["size"]) {
        usage = usage.with_context_size(tokens);
    }
    if let Some(cost) = value.get("cost") {
        let amount = cost.get("amount").and_then(number_text);
        let currency = cost.get("currency").and_then(Value::as_str);
        if let (Some(amount), Some(currency)) = (amount, currency) {
            usage = usage.with_cost(amount, currency);
        }
    }
    usage
}

fn from_tokens(value: &Value) -> ReportedUsage {
    let mut usage = ReportedUsage::new();
    if let Some(tokens) = number(value, &["inputTokens", "input_tokens"]) {
        usage = usage.with_input_tokens(tokens);
    }
    if let Some(tokens) = number(value, &["outputTokens", "output_tokens"]) {
        usage = usage.with_output_tokens(tokens);
    }
    if let Some(tokens) = number(value, &["thoughtTokens", "reasoningTokens"]) {
        usage = usage.with_reasoning_tokens(tokens);
    }
    if let Some(tokens) = number(
        value,
        &[
            "cachedReadTokens",
            "cacheReadTokens",
            "cache_read_input_tokens",
        ],
    ) {
        usage = usage.with_cache_read_tokens(tokens);
    }
    usage
}

fn merge(base: ReportedUsage, next: ReportedUsage) -> ReportedUsage {
    base.merged(next)
}

fn number(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| value.get(*key).and_then(as_u64))
}

fn as_u64(value: &Value) -> Option<u64> {
    let number = value.as_number()?;
    number
        .as_u64()
        .or_else(|| number.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn number_text(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}
