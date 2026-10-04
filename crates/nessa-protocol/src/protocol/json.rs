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
//! own names and carries what is nested without choosing from it. A string that
//! is not Unicode does not hide those names.

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
///
/// A string serde_json cannot turn into Unicode — a lone UTF-16 surrogate — is
/// not text. The walk leaves JSON null in its place and continues, so an `id`
/// that itself is Unicode can still be read. Callers read `type` and `id` only
/// when they are strings; that null is not the sender's value.
pub fn unique_envelope(text: &str) -> Result<Value, serde_json::Error> {
    let mut parser = EnvelopeParser {
        text,
        index: 0,
        budget: JsonBudget {
            remaining: MAX_JSON_ITEMS,
        },
    };
    parser.skip_ws();
    parser.budget.take()?;
    let depth = deeper(0)?;
    parser.expect(b'{')?;
    let value = Value::Object(parser.object_body(depth, true)?);
    parser.skip_ws();
    if parser.index != parser.text.len() {
        return Err(syntax("trailing data"));
    }
    Ok(value)
}

fn syntax(message: &str) -> serde_json::Error {
    Error::custom(message)
}

// serde_json's default recursion limit fails the 128th container. Pinned by
// `envelope_container_depth_matches_serde_json`.
const MAX_CONTAINERS: usize = 127;

fn deeper(depth: usize) -> Result<usize, serde_json::Error> {
    if depth >= MAX_CONTAINERS {
        return Err(syntax("recursion limit exceeded"));
    }
    Ok(depth + 1)
}

struct EnvelopeParser<'a> {
    text: &'a str,
    index: usize,
    budget: JsonBudget,
}

impl<'a> EnvelopeParser<'a> {
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.index).copied()
    }

    fn pop_byte(&mut self) -> Result<u8, serde_json::Error> {
        match self.peek() {
            Some(byte) => {
                self.index += 1;
                Ok(byte)
            }
            None => Err(syntax("unexpected end of JSON")),
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), serde_json::Error> {
        if self.pop_byte()? != byte {
            return Err(syntax("unexpected JSON byte"));
        }
        Ok(())
    }

    fn object_body(
        &mut self,
        depth: usize,
        envelope: bool,
    ) -> Result<Map<String, Value>, serde_json::Error> {
        let mut values = Map::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.index += 1;
            return Ok(values);
        }
        loop {
            self.skip_ws();
            self.budget.take()?;
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            if envelope {
                if let Some(name) = &key {
                    if values.contains_key(name) {
                        return Err(syntax("duplicate frame envelope key"));
                    }
                }
            }
            // Nested values are carried, never chosen from — a repeated name
            // below the envelope is kept rather than refused. They are counted
            // like any other item; charging one for a whole subtree would let
            // the correlation path allocate what the strict path refuses.
            let value = self.parse_value(depth)?;
            if let Some(key) = key {
                values.insert(key, value);
            }
            self.skip_ws();
            match self.pop_byte()? {
                b',' => continue,
                b'}' => return Ok(values),
                _ => return Err(syntax("expected ',' or '}'")),
            }
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, serde_json::Error> {
        self.budget.take()?;
        self.skip_ws();
        match self.peek() {
            Some(b'{') => {
                let depth = deeper(depth)?;
                self.expect(b'{')?;
                Ok(Value::Object(self.object_body(depth, false)?))
            }
            Some(b'[') => {
                let depth = deeper(depth)?;
                self.expect(b'[')?;
                Ok(Value::Array(self.array_body(depth)?))
            }
            Some(b'"') => Ok(match self.parse_string()? {
                Some(text) => Value::String(text),
                // Not the sender's text. A request id that lands here is not a
                // string, so correlation does not answer it.
                None => Value::Null,
            }),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(syntax("invalid JSON value")),
        }
    }

    fn array_body(&mut self, depth: usize) -> Result<Vec<Value>, serde_json::Error> {
        let mut values = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.index += 1;
            return Ok(values);
        }
        loop {
            values.push(self.parse_value(depth)?);
            self.skip_ws();
            match self.pop_byte()? {
                b',' => continue,
                b']' => return Ok(values),
                _ => return Err(syntax("expected ',' or ']'")),
            }
        }
    }

    fn literal(&mut self, bytes: &[u8], value: Value) -> Result<Value, serde_json::Error> {
        if !self.text.as_bytes()[self.index..].starts_with(bytes) {
            return Err(syntax("invalid literal"));
        }
        self.index += bytes.len();
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Value, serde_json::Error> {
        let start = self.index;
        if self.peek() == Some(b'-') {
            self.index += 1;
        }
        match self.peek() {
            Some(b'0') => self.index += 1,
            Some(b'1'..=b'9') => {
                self.index += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.index += 1;
                }
            }
            _ => return Err(syntax("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.index += 1;
            let fraction = self.index;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
            if self.index == fraction {
                return Err(syntax("invalid number"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            let exponent = self.index;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.index += 1;
            }
            if self.index == exponent {
                return Err(syntax("invalid number"));
            }
        }
        serde_json::from_str(&self.text[start..self.index]).map_err(|_| syntax("invalid number"))
    }

    /// `Some` is a Unicode string. `None` is a structurally valid string that is
    /// not Unicode (a lone surrogate); the caller keeps walking.
    fn parse_string(&mut self) -> Result<Option<String>, serde_json::Error> {
        self.expect(b'"')?;
        let mut out = String::new();
        let mut unicode = true;
        loop {
            match self.peek() {
                None => return Err(syntax("unterminated string")),
                Some(b'"') => {
                    self.index += 1;
                    break;
                }
                Some(b'\\') => {
                    self.index += 1;
                    if !self.escape(unicode.then_some(&mut out))? {
                        unicode = false;
                        out.clear();
                    }
                }
                Some(0x00..=0x1F) => return Err(syntax("control character in string")),
                Some(_) => {
                    let ch = self.pop_char()?;
                    if unicode {
                        out.push(ch);
                    }
                }
            }
        }
        Ok(unicode.then_some(out))
    }

    fn pop_char(&mut self) -> Result<char, serde_json::Error> {
        let ch = self.text[self.index..]
            .chars()
            .next()
            .ok_or_else(|| syntax("unexpected end of JSON"))?;
        self.index += ch.len_utf8();
        Ok(ch)
    }

    /// `out` is `None` once the string is already not Unicode, so the rest is
    /// only scanned. Returns whether this escape is a Unicode scalar. A lone
    /// surrogate returns false and leaves the next byte of the string in place:
    /// serde_json stops at that byte, and stopping here would hide a later `id`.
    fn escape(&mut self, mut out: Option<&mut String>) -> Result<bool, serde_json::Error> {
        let simple = match self.pop_byte()? {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{0008}',
            b'f' => '\u{000c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(out),
            _ => return Err(syntax("invalid escape")),
        };
        if let Some(out) = out.as_mut() {
            out.push(simple);
        }
        Ok(true)
    }

    fn unicode_escape(&mut self, mut out: Option<&mut String>) -> Result<bool, serde_json::Error> {
        let unit = self.hex4()?;
        if (0xDC00..=0xDFFF).contains(&unit) {
            return Ok(false);
        }
        if !(0xD800..=0xDBFF).contains(&unit) {
            if let Some(out) = out.as_mut() {
                out.push(char::from_u32(u32::from(unit)).ok_or_else(|| syntax("invalid unicode"))?);
            }
            return Ok(true);
        }
        if self.peek() != Some(b'\\') || self.text.as_bytes().get(self.index + 1) != Some(&b'u') {
            return Ok(false);
        }
        self.index += 2;
        let low = self.hex4()?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return Ok(false);
        }
        if let Some(out) = out.as_mut() {
            let scalar = 0x1_0000 + ((u32::from(unit - 0xD800) << 10) | u32::from(low - 0xDC00));
            out.push(char::from_u32(scalar).ok_or_else(|| syntax("invalid unicode"))?);
        }
        Ok(true)
    }

    fn hex4(&mut self) -> Result<u16, serde_json::Error> {
        let mut unit = 0u16;
        for _ in 0..4 {
            let digit = match self.pop_byte()? {
                byte @ b'0'..=b'9' => byte - b'0',
                byte @ b'a'..=b'f' => byte - b'a' + 10,
                byte @ b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(syntax("invalid hex escape")),
            };
            unit = (unit << 4) | u16::from(digit);
        }
        Ok(unit)
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
    fn a_decodable_envelope_matches_serde_json() {
        for text in [
            r#"{"type":"req","id":"x","method":"m","params":{"scope":["a","b"],"nested":{"x":1,"y":null}},"flag":true,"n":null,"count":10,"list":[1,"a",false]}"#,
            r#"{"type":"req","id":"x","method":"m","params":{"p":1,"p":2,"e":"\ud83d\ude00","slash":"\/"},"n":-0,"exp":1e2}"#,
            r#"{"id":"x","method":"m","params":{}}"#,
            r#"{}"#,
            r#"{"a":"\u0000\n\t\/"}"#,
        ] {
            assert_eq!(
                unique_envelope(text).unwrap(),
                serde_json::from_str::<Value>(text).unwrap(),
                "{text}"
            );
        }
        for text in [
            r#"{"a":01}"#,
            r#"{"a":1.}"#,
            r#"{"a":-}"#,
            r#"{"a":1e}"#,
            r#"{"a":+1}"#,
            r#"{"a":"unterminated}"#,
            "{\"a\":\"\n\"}",
        ] {
            assert_eq!(
                unique_envelope(text).is_err(),
                serde_json::from_str::<Value>(text).is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn an_undecodable_string_does_not_hide_a_readable_request_id() {
        // Both serde_json failures: a lone leading surrogate (`\ud800`,
        // unexpected end of the pair) and a lone trailing one (`\udfff`).
        for text in [
            r#"{"type":"req","id":"request-9","method":"mcp.callTool","params":{"argumentsJson":"\ud800"}}"#,
            r#"{"type":"req","method":"server.health","params":{"note":"\ud800"},"id":"request-9"}"#,
            r#"{"params":{"note":"\udfff"},"id":"request-9","method":"conversation.send","type":"req"}"#,
            r#"{"type":"req","id":"request-9","method":"mcp.readResource","params":{"uri":"\uD800\uD800"}}"#,
            r#"{"type":"req","\ud800":1,"id":"request-9","method":"m","params":{}}"#,
            r#"{"type":"req","method":"\ud800","id":"request-9","params":{}}"#,
        ] {
            let value = unique_envelope(text).unwrap_or_else(|error| panic!("{text}: {error}"));
            assert_eq!(value["id"], "request-9", "{text}");
            assert_eq!(value["type"], "req", "{text}");
            assert!(unique_value(text).is_err(), "strict decode accepted {text}");
        }
        // The id itself is not Unicode: there is no request id to answer.
        let bare =
            unique_envelope(r#"{"type":"req","id":"\ud800","method":"m","params":{}}"#).unwrap();
        assert!(bare.get("id").and_then(Value::as_str).is_none());
        // A repeated id is still not one request, including when spelled as an escape.
        for text in [
            r#"{"type":"req","id":"a","id":"b","method":"m","params":{"a":"\ud800"}}"#,
            r#"{"id":"\ud800","id":"real"}"#,
            r#"{"id":"real","id":"\ud800"}"#,
            r#"{"id":"a","\u0069\u0064":"b"}"#,
            r#"{"\u0069\u0064":"b","id":"a"}"#,
        ] {
            assert!(unique_envelope(text).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn envelope_container_depth_matches_serde_json() {
        for depth in [126, 127, 128, 129] {
            let mut nested = "0".to_string();
            for _ in 0..depth {
                nested = format!("[{nested}]");
            }
            let frame = format!(r#"{{"id":{nested}}}"#);
            assert_eq!(
                unique_envelope(&frame).is_ok(),
                serde_json::from_str::<Value>(&frame).is_ok(),
                "depth {depth}"
            );
        }
    }

    #[test]
    fn the_envelope_decode_rejects_a_non_object_and_trailing_input() {
        assert!(unique_envelope("null").is_err());
        assert!(unique_envelope("[1]").is_err());
        assert!(unique_envelope("{").is_err());
        assert!(unique_envelope(r#"{"id":"x"} {"id":"y"}"#).is_err());
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
