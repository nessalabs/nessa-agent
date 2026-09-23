//! Bounded, non-retaining preflight before allocating an owned journal record.
//!
//! Serde handles JSON grammar and Unicode. A token reader bounds its scratch
//! allocation before visitor callbacks; seeds bound decoded fields, collections,
//! and diagnostic trees. Each changed invocation has its own allocation budget,
//! so a journal checkpoint can still contain arbitrarily many valid prior turns.
mod indices;
mod reader;
mod shape;
use crate::application::agent_execution::sessions::StorageError;
use indices::Indices;
use reader::TokenReader;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use shape::{Shape, ERROR_BYTES, KEY_BYTES};
use std::{cell::Cell, fmt, io::Read, rc::Rc};

// The retained output envelope is 128 MiB, with a 4 MiB prompt and independent
// bounded provider/cleanup diagnostics. Count decoded text and structural slots
// here; final typed validation enforces actual retained capacities and payloads.
const CHANGE_BYTES: usize = 160 * 1024 * 1024;
#[derive(Default)]
struct Budget {
    bytes: Cell<usize>,
    nodes: Cell<usize>,
}
impl Budget {
    fn add<E: de::Error>(
        &self,
        bytes: usize,
        nodes: usize,
        max_bytes: usize,
        max_nodes: usize,
    ) -> Result<(), E> {
        let bytes = self.bytes.get().saturating_add(bytes);
        let nodes = self.nodes.get().saturating_add(nodes);
        if bytes > max_bytes || nodes > max_nodes {
            return Err(E::custom("journal value exceeds decoding budget"));
        }
        self.bytes.set(bytes);
        self.nodes.set(nodes);
        Ok(())
    }
}
struct Seed {
    shape: Shape,
    limit: Rc<Cell<usize>>,
    indices: Rc<Indices>,
    change: Option<Rc<Budget>>,
    metadata_present: Option<Rc<Cell<bool>>>,
    error: Option<Rc<Budget>>,
    error_depth: usize,
}
impl Seed {
    fn child(&self, shape: Shape) -> Self {
        let new_change = matches!(shape, Shape::Change | Shape::Reorder);
        let error = if matches!(shape, Shape::Error) {
            Some(self.error.clone().unwrap_or_default())
        } else {
            self.error.clone()
        };
        Self {
            shape,
            limit: self.limit.clone(),
            indices: self.indices.clone(),
            change: if new_change {
                Some(Rc::default())
            } else {
                self.change.clone()
            },
            metadata_present: if new_change {
                Some(Rc::new(Cell::new(false)))
            } else {
                self.metadata_present.clone()
            },
            error,
            error_depth: self.error_depth + usize::from(matches!(shape, Shape::Error)),
        }
    }
    fn charge<E: de::Error>(&self, bytes: usize) -> Result<(), E> {
        if let Some(change) = &self.change {
            change.add(bytes.saturating_add(8), 1, CHANGE_BYTES, CHANGE_BYTES / 8)?;
        }
        if let Some(error) = &self.error {
            error.add(bytes, 0, ERROR_BYTES, 128)?;
        }
        Ok(())
    }
    fn error_node<E: de::Error>(&self) -> Result<(), E> {
        if matches!(self.shape, Shape::Error | Shape::Hook) {
            if self.error_depth > 32 {
                return Err(E::custom("journal diagnostic exceeds depth 32"));
            }
            if let Some(error) = &self.error {
                error.add(0, 1, ERROR_BYTES, 128)?;
            }
        }
        Ok(())
    }
}
impl<'de> DeserializeSeed<'de> for Seed {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        let mut limit = self.shape.string_limit();
        if let Some(change) = &self.change {
            limit = limit.min(CHANGE_BYTES.saturating_sub(change.bytes.get()));
        }
        if let Some(error) = &self.error {
            limit = limit.min(ERROR_BYTES.saturating_sub(error.bytes.get()));
        }
        self.limit.set(limit);
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded journal value")
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        self.charge(0)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        self.charge(0)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        self.charge(0)
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<(), E> {
        match self.shape {
            Shape::Index | Shape::Count => {
                let value = usize::try_from(value)
                    .map_err(|_| E::custom("journal integer exceeds platform size"))?;
                if matches!(self.shape, Shape::Index) {
                    self.indices.index(value)?;
                } else {
                    self.indices.count(value);
                }
            }
            _ => {}
        }
        self.charge(0)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        self.charge(0)
    }
    fn visit_str<E: de::Error>(self, text: &str) -> Result<(), E> {
        if text.len() > self.shape.string_limit() {
            return Err(E::custom("journal field exceeds decoded string limit"));
        }
        self.error_node()?;
        self.charge(text.len())
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<(), M::Error> {
        self.error_node()?;
        self.charge(0)?;
        let mut fields = 0;
        let mut has_index = false;
        let mut has_provider_diagnostic = false;
        if matches!(self.shape, Shape::Metadata) {
            if let Some(present) = &self.metadata_present {
                present.set(true);
            }
        }
        loop {
            self.limit.set(KEY_BYTES);
            let Some(key) = map.next_key_seed(Key)? else {
                if matches!(self.shape, Shape::Change)
                    && (!has_index
                        || (self.indices.needs_metadata()
                            && !self
                                .metadata_present
                                .as_ref()
                                .is_some_and(|present| present.get())))
                {
                    return Err(de::Error::custom(
                        "journal change has no index or new metadata",
                    ));
                }
                if matches!(self.shape, Shape::Record) {
                    self.indices.finish()?;
                }
                if matches!(self.shape, Shape::ProviderError) && !has_provider_diagnostic {
                    return Err(de::Error::custom(
                        "saved provider error has no diagnostic field",
                    ));
                }
                return Ok(());
            };
            if !self.shape.allows(&key) {
                return Err(de::Error::custom(
                    "journal value has an unknown field for its schema",
                ));
            }
            has_index |= key == "index";
            has_provider_diagnostic |= key == "diagnostic";
            fields += 1;
            if fields > 32 {
                return Err(de::Error::custom("journal object has too many fields"));
            }
            map.next_value_seed(self.child(self.shape.field(&key)))?;
        }
    }
    fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<(), S::Error> {
        self.charge(0)?;
        let mut count = 0usize;
        loop {
            // The rejecting seed checks before deserializing the next value,
            // rather than allowing Serde to build one oversized excess element.
            let child = self.child(self.shape.element());
            if sequence
                .next_element_seed(Element {
                    child,
                    reject: count >= self.shape.array_limit(),
                })?
                .is_none()
            {
                if matches!(self.shape, Shape::Reorders) {
                    self.indices.queue_events(count);
                }
                return Ok(());
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| de::Error::custom("journal collection overflow"))?;
        }
    }
}
struct Element {
    child: Seed,
    reject: bool,
}
impl<'de> DeserializeSeed<'de> for Element {
    type Value = ();
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.reject {
            return Err(de::Error::custom(
                "journal collection exceeds decoding limit",
            ));
        }
        self.child.deserialize(deserializer)
    }
}
struct Key;
impl<'de> DeserializeSeed<'de> for Key {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_str(self)
    }
}
impl Visitor<'_> for Key {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded journal field name")
    }
    fn visit_str<E: de::Error>(self, text: &str) -> Result<String, E> {
        if text.len() > KEY_BYTES {
            return Err(E::custom("journal field name exceeds decoding limit"));
        }
        Ok(text.into())
    }
}
pub(super) fn preflight(
    reader: impl Read,
    previous_invocations: usize,
) -> Result<(), StorageError> {
    let limit = Rc::new(Cell::new(KEY_BYTES));
    let reader = TokenReader::new(reader, limit.clone());
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    Seed {
        shape: Shape::Record,
        limit,
        indices: Rc::new(Indices::new(previous_invocations)),
        change: None,
        metadata_present: None,
        error: None,
        error_depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| StorageError::Corrupt(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| StorageError::Corrupt(error.to_string()))
}
