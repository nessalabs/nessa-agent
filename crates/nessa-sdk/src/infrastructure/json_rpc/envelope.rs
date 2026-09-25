use super::protocol;
use crate::application::agent_execution::agents::AgentError;
use serde::{
    de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::{json, Map, Number, Value};
use std::fmt;
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub(crate) enum RpcId {
    Number(i64),
    Text(String),
}
impl RpcId {
    pub fn value(&self) -> Value {
        match self {
            Self::Number(id) => json!(id),
            Self::Text(id) => json!(id),
        }
    }
}
#[derive(Deserialize)]
pub(crate) struct Envelope {
    pub jsonrpc: String,
    pub id: Option<RpcId>,
    pub method: Option<String>,
    pub params: Option<Value>,
    #[serde(default, deserialize_with = "present_result")]
    pub result: Option<Value>,
    pub error: Option<RpcError>,
}
#[derive(Deserialize)]
pub(crate) struct RpcError {
    pub code: i64,
    /// The provider's own explanation, kept for the log and nothing else.
    ///
    /// Never matched on: which error this is remains [`Self::code`]'s to say, so
    /// a provider rewording its text cannot change how Nessa behaves. But the
    /// code alone is what an operator is left holding, and a bare `-32000` does
    /// not tell them Codex is simply not signed in. Read here so the adapter has
    /// something to report; classification stays with the number.
    #[serde(default)]
    pub message: Option<String>,
}
/// [`parse_within`] with the protocol's usual budget, [`MAX_JSON_ITEMS`].
#[cfg(test)]
pub(crate) fn parse(frame: &[u8]) -> Result<Envelope, AgentError> {
    parse_within(frame, MAX_JSON_ITEMS)
}
/// Parse one frame, charging each value and key against a budget of `items`.
pub(crate) fn parse_within(frame: &[u8], items: usize) -> Result<Envelope, AgentError> {
    let mut decoder = serde_json::Deserializer::from_slice(frame);
    let mut budget = JsonBudget { remaining: items };
    let value = UniqueValue(&mut budget)
        .deserialize(&mut decoder)
        .map_err(|_| protocol("invalid or over-budget JSON-RPC frame"))?;
    decoder
        .end()
        .map_err(|_| protocol("invalid JSON-RPC frame"))?;
    let message: Envelope =
        serde_json::from_value(value).map_err(|_| protocol("invalid JSON-RPC frame"))?;
    if matches!(&message.id, Some(RpcId::Text(id)) if id.is_empty() || id.len() > 256) {
        return Err(protocol("invalid RPC identifier"));
    }
    if message.jsonrpc != "2.0"
        || (message.method.is_some() && (message.result.is_some() || message.error.is_some()))
        || (message.method.is_none()
            && (message.id.is_none() || message.result.is_some() == message.error.is_some()))
    {
        return Err(protocol("invalid JSON-RPC envelope"));
    }
    Ok(message)
}
pub(crate) fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
pub(crate) fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","method":method,"params":params})
}
pub(crate) fn unsupported(id: &RpcId) -> Value {
    json!({"jsonrpc":"2.0","id":id.value(),"error":{"code":-32601,"message":"Client operation is not enabled"}})
}
pub(crate) fn success(id: &RpcId, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id.value(),"result":result})
}

// A successful JSON-RPC result may be null; distinguish it from an absent field.
fn present_result<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

// A frame's byte limit bounds strings, but tiny collections can allocate far more
// than their wire size. Count every value and object key before allocating it.
pub(crate) const MAX_JSON_ITEMS: usize = 65_536;
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

// Decode objects before serde_json::Value can collapse repeated names. The same
// budget and duplicate checks cover unknown fields and objects inside arrays.
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
    #[test]
    fn malformed_envelopes_and_oversized_identifiers_cannot_enter_dispatch() {
        for frame in [
            json!({"jsonrpc":"1.0","id":1,"result":{}}),
            json!({"jsonrpc":"2.0","result":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":0}}),
            json!({"jsonrpc":"2.0","id":1,"method":"x","result":{}}),
            json!({"jsonrpc":"2.0","id":"x".repeat(257),"result":{}}),
        ] {
            assert!(parse(&serde_json::to_vec(&frame).unwrap()).is_err());
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/infrastructure/json_rpc/decoding_budget.rs"]
mod decoding_budget;
