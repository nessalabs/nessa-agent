//! JSON decoding for client frames, before a map can hide what arrived.
//!
//! `serde_json::Value` keeps the last of two entries with the same name, so a
//! frame carrying `credential` twice reaches method validation as a single
//! agreed value. Nothing downstream can tell that apart from a frame that only
//! ever said it once. [`unique_value`] rejects repeated names instead, at every
//! depth and whichever entry a sender hoped would win.
//!
//! [`unique_envelope`] is the narrower rule for a frame that has already been
//! rejected and only has to be attributed to a request: it judges the envelope's
//! own names and carries what is nested without choosing from it.

use serde::{
    de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Decode `text` into a value whose objects have unique keys at every depth.
///
/// Returns the decode error for malformed JSON, trailing content, a repeated
/// object key, or more items than the budget allows. The caller maps it to its
/// own boundary failure.
pub fn unique_value(text: &str) -> Result<Value, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    let mut budget = JsonBudget {
        remaining: MAX_JSON_ITEMS,
    };
    let value = UniqueValue(&mut budget).deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

/// Decode `text` as a frame envelope, judging only the envelope's own key names.
///
/// For deciding which request an already-rejected frame belongs to. A nested
/// duplicate does not make `id` ambiguous, so such a frame can still be answered;
/// a frame that names `id` or `method` twice cannot, and is rejected here rather
/// than having the server pick one. Nested values are decoded as they arrive and
/// counted against the same budget; nothing is selected from them.
pub fn unique_envelope(text: &str) -> Result<Value, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    let mut budget = JsonBudget {
        remaining: MAX_JSON_ITEMS,
    };
    let value = EnvelopeValue(&mut budget).deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

struct EnvelopeValue<'a>(&'a mut JsonBudget);
impl<'de> DeserializeSeed<'de> for EnvelopeValue<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.0.take()?;
        deserializer.deserialize_any(EnvelopeVisitor(self.0))
    }
}
struct EnvelopeVisitor<'a>(&'a mut JsonBudget);
// Only `visit_map` is implemented: an envelope that is not an object is not a
// request and has no `id` to answer to, so it is refused here.
impl<'de> Visitor<'de> for EnvelopeVisitor<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a frame envelope with unique key names")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key_seed(ObjectKey(self.0))? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate frame envelope key"));
            }
            // Nested values are carried, never chosen from — so a repeated name
            // below the envelope is kept rather than refused. They are counted
            // like any other item; charging one for a whole subtree would let
            // the correlation path allocate what the strict path refuses.
            values.insert(key, map.next_value_seed(NestedValue(self.0))?);
        }
        Ok(Value::Object(values))
    }
}
struct NestedValue<'a>(&'a mut JsonBudget);
impl<'de> DeserializeSeed<'de> for NestedValue<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.0.take()?;
        deserializer.deserialize_any(NestedVisitor(self.0))
    }
}
struct NestedVisitor<'a>(&'a mut JsonBudget);
impl<'de> Visitor<'de> for NestedVisitor<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON")
    }
    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(NestedValue(self.0))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key_seed(ObjectKey(self.0))? {
            values.insert(key, map.next_value_seed(NestedValue(self.0))?);
        }
        Ok(Value::Object(values))
    }
}

// Small collections allocate far more than they take on the wire: two bytes of
// `[0,0,0,...]` buy a whole `Value`, which is tens of bytes. Count every value
// and key before allocating it. The exact figures are pinned by
// `the_item_budget_stays_a_backstop_behind_the_frame_byte_limit` rather than
// written here, where they would quietly rot.
//
// At the socket this is a backstop rather than the binding constraint: two bytes
// is the densest an item can be written, so `MAX_PAYLOAD_BYTES` already caps a
// frame at half this count. It binds for any caller that is not behind that
// limit, and it keeps this decoder's shape the same as the one in `nessa-sdk`'s
// `infrastructure/json_rpc/envelope.rs` — separate because neither crate depends
// on the other's internals, and meant to behave alike.
const MAX_JSON_ITEMS: usize = 65_536;
struct JsonBudget {
    remaining: usize,
}
impl JsonBudget {
    fn take<E: Error>(&mut self) -> Result<(), E> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| E::custom("JSON item limit exceeded"))?;
        Ok(())
    }
}

struct UniqueValue<'a>(&'a mut JsonBudget);
impl<'de> DeserializeSeed<'de> for UniqueValue<'_> {
    type Value = Value;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.0.take()?;
        deserializer.deserialize_any(UniqueValueVisitor(self.0))
    }
}
struct ObjectKey<'a>(&'a mut JsonBudget);
impl<'de> DeserializeSeed<'de> for ObjectKey<'_> {
    type Value = String;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        self.0.take()?;
        String::deserialize(deserializer)
    }
}
struct UniqueValueVisitor<'a>(&'a mut JsonBudget);
impl<'de> Visitor<'de> for UniqueValueVisitor<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }
    fn visit_bool<E: Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("invalid JSON number"))
    }
    fn visit_str<E: Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_unit<E: Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    // Objects inside arrays are checked by the same rule as objects at the top.
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(UniqueValue(self.0))? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = map.next_key_seed(ObjectKey(self.0))? {
            if values.contains_key(&key) {
                return Err(A::Error::custom("duplicate JSON object key"));
            }
            values.insert(key, map.next_value_seed(UniqueValue(self.0))?);
        }
        Ok(Value::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repeated_names_are_rejected_at_every_depth_and_in_both_orders() {
        for text in [
            r#"{"type":"req","type":"req"}"#,
            r#"{"params":{"credential":"a","credential":"b"}}"#,
            r#"{"params":{"credential":"b","credential":"a"}}"#,
            r#"{"params":{"credential":"same","credential":"same"}}"#,
            r#"{"params":{"grant":{"resource":{"id":"a","id":"b"}}}}"#,
            r#"{"params":{"members":[{"role":"member","role":"admin"}]}}"#,
        ] {
            assert!(unique_value(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn ordinary_frames_decode_unchanged() {
        let value = unique_value(
            r#"{"type":"req","id":"1","method":"health","params":{"scope":["a","b"],"nested":{"x":1,"y":null}}}"#,
        )
        .unwrap();
        assert_eq!(value["method"], "health");
        assert_eq!(value["params"]["scope"][1], "b");
        assert!(value["params"]["nested"]["y"].is_null());
    }

    #[test]
    fn malformed_and_trailing_input_is_rejected() {
        assert!(unique_value("{").is_err());
        assert!(unique_value(r#"{"a":1} {"a":2}"#).is_err());
    }

    #[test]
    fn keys_are_compared_after_unescaping_and_at_the_frame_level() {
        // Plain `serde_json` collapses these to one entry; the names are equal
        // once decoded, whichever way they were spelled on the wire.
        assert!(unique_value(r#"{"a":1,"\u0061":2}"#).is_err());
        assert!(unique_value(r#"{"\u0061":1,"a":2}"#).is_err());
        // Correlation and dispatch keys are policy-bearing too.
        for text in [
            r#"{"type":"req","id":"a","id":"b","method":"health","params":{}}"#,
            r#"{"type":"req","id":"a","method":"health","method":"other","params":{}}"#,
            r#"{"type":"req","id":"a","method":"health","params":{},"params":{}}"#,
        ] {
            assert!(unique_value(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn non_objects_and_excessive_depth_are_handled_without_panicking() {
        // A top-level non-object decodes; rejecting it is the frame's job.
        assert_eq!(unique_value("[1,2]").unwrap(), json!([1, 2]));
        assert_eq!(unique_value("null").unwrap(), Value::Null);
        // serde_json's own recursion limit fires before the visitor recurses away.
        assert!(unique_value(&"[".repeat(200)).is_err());
    }

    #[test]
    fn the_envelope_decode_counts_nested_items_like_the_strict_one() {
        // Correlating a rejected frame must not be a way to allocate what the
        // strict decode refuses.
        let nested = format!(
            r#"{{"type":"req","id":"x","method":"m","params":[{}]}}"#,
            vec!["0"; MAX_JSON_ITEMS + 1].join(",")
        );
        assert!(unique_envelope(&nested).is_err());
        assert!(unique_value(&nested).is_err());
    }

    #[test]
    fn the_envelope_decode_judges_only_the_envelopes_own_names() {
        // A nested duplicate still leaves one `id` to answer to.
        let nested = r#"{"type":"req","id":"x","method":"m","params":{"p":1,"p":2}}"#;
        assert_eq!(unique_envelope(nested).unwrap()["id"], "x");
        assert!(unique_value(nested).is_err());
        // The envelope's own repeated names are refused, in both orders.
        for text in [
            r#"{"type":"req","id":"a","id":"b","method":"m","params":{}}"#,
            r#"{"type":"req","id":"b","id":"a","method":"m","params":{}}"#,
            r#"{"type":"req","id":"a","method":"m","method":"n","params":{}}"#,
        ] {
            assert!(unique_envelope(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn the_item_budget_stays_a_backstop_behind_the_frame_byte_limit() {
        // The module comment claims a frame cannot reach this budget, because
        // two bytes is the densest an item can be written. Pin that arithmetic:
        // raising either constant without the other would make the claim false.
        let densest_items = (super::super::encode::MAX_PAYLOAD_BYTES as usize).div_ceil(2);
        assert!(
            densest_items < MAX_JSON_ITEMS,
            "a frame can now reach the budget: {densest_items} items in \
             {} bytes",
            super::super::encode::MAX_PAYLOAD_BYTES
        );
        // And the budget is still worth having for callers that are not behind
        // that limit: what it admits has to stay a bound rather than grow into
        // an unbounded backstop. Raising `MAX_JSON_ITEMS` is what this catches.
        let retained = MAX_JSON_ITEMS * std::mem::size_of::<Value>();
        assert!(
            retained <= 8 * 1024 * 1024,
            "the budget now admits {retained} bytes of values"
        );
    }

    #[test]
    fn a_collection_larger_than_the_item_budget_is_rejected_before_allocation() {
        let within = format!("[{}]", vec!["0"; MAX_JSON_ITEMS - 1].join(","));
        assert!(unique_value(&within).is_ok());
        let beyond = format!("[{}]", vec!["0"; MAX_JSON_ITEMS + 1].join(","));
        assert!(unique_value(&beyond).is_err());
        // Keys count against the same budget as values.
        let keys: Vec<_> = (0..=MAX_JSON_ITEMS)
            .map(|index| format!("\"{index}\":0"))
            .collect();
        assert!(unique_value(&format!("{{{}}}", keys.join(","))).is_err());
    }
}
