//! Bounded, non-retaining preflight before allocating an owned semantic fact.
//!
//! Serde handles JSON grammar and Unicode. A token reader bounds its scratch
//! allocation before visitor callbacks; seeds bound decoded fields, collections,
//! and diagnostic trees.
mod reader;
mod shape;
use crate::application::agent_execution::{agents::DiagnosticTreeLimits, sessions::StorageError};
use reader::TokenReader;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use shape::{Shape, ERROR_BYTES, KEY_BYTES};
use std::{cell::Cell, fmt, io::Read, rc::Rc};

// The retained output envelope is 128 MiB, with a 4 MiB prompt and independent
// bounded provider/cleanup diagnostics. Count decoded text and structural slots
// here; final typed validation enforces actual retained capacities and payloads.
const CHANGE_BYTES: usize = 160 * 1024 * 1024;
const FINALIZED_RECIPE_BYTES: usize = 2 * 128 * ERROR_BYTES;
const FINALIZED_RECIPE_NODES: usize = 2 * 128 * 128;
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
    change: Option<Rc<Budget>>,
    error: Option<Rc<Budget>>,
    recipe: Option<Rc<Budget>>,
    error_depth: usize,
    storage: Option<Rc<Budget>>,
    storage_node: bool,
}
impl Seed {
    fn child(&self, shape: Shape) -> Self {
        let new_change = matches!(shape, Shape::Reorder);
        let storage_node = matches!(shape, Shape::StorageError)
            && (self.error.is_none() || matches!(self.shape, Shape::StorageChildren));
        let error = if matches!(shape, Shape::Error | Shape::StorageError) {
            Some(self.error.clone().unwrap_or_default())
        } else {
            self.error.clone()
        };
        let recipe = if matches!(shape, Shape::FinalizedComponents) {
            Some(Rc::default())
        } else {
            self.recipe.clone()
        };
        Self {
            shape,
            limit: self.limit.clone(),
            change: if new_change {
                Some(Rc::default())
            } else {
                self.change.clone()
            },
            error,
            recipe,
            error_depth: self.error_depth
                + usize::from(matches!(shape, Shape::Error) || storage_node),
            // Plain storage leaves use their enclosing AgentError budget. Only
            // failed acknowledgements and shutdown aggregates establish the
            // storage diagnostic budget; nested aggregates share it.
            storage: if matches!(shape, Shape::StorageChildren)
                || (matches!(shape, Shape::StorageError)
                    && matches!(self.shape, Shape::FailedAcknowledgement))
            {
                Some(self.storage.clone().unwrap_or_default())
            } else if matches!(shape, Shape::StorageError | Shape::StorageDiagnostic) {
                self.storage.clone()
            } else {
                None
            },
            storage_node,
        }
    }
    fn charge<E: de::Error>(&self, bytes: usize) -> Result<(), E> {
        if let Some(change) = &self.change {
            change.add(bytes.saturating_add(8), 1, CHANGE_BYTES, CHANGE_BYTES / 8)?;
        }
        if let Some(error) = &self.error {
            error.add(bytes, 0, ERROR_BYTES, DiagnosticTreeLimits::NODES)?;
        }
        if matches!(self.shape, Shape::StorageDiagnostic) {
            if let Some(storage) = &self.storage {
                storage.add(bytes, 0, StorageError::DIAGNOSTIC_BYTES, usize::MAX)?;
            }
        }
        if let Some(recipe) = &self.recipe {
            recipe.add(
                bytes.saturating_add(8),
                1,
                FINALIZED_RECIPE_BYTES,
                FINALIZED_RECIPE_NODES,
            )?;
        }
        Ok(())
    }
    fn error_node<E: de::Error>(&self) -> Result<(), E> {
        if matches!(self.shape, Shape::Error | Shape::Hook) || self.storage_node {
            if self.error_depth > DiagnosticTreeLimits::DEPTH {
                return Err(E::custom("journal diagnostic exceeds depth budget"));
            }
            if let Some(error) = &self.error {
                error.add(0, 1, ERROR_BYTES, DiagnosticTreeLimits::NODES)?;
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
        if let Some(recipe) = &self.recipe {
            limit = limit.min(FINALIZED_RECIPE_BYTES.saturating_sub(recipe.bytes.get()));
        }
        if matches!(self.shape, Shape::StorageDiagnostic) {
            if let Some(storage) = &self.storage {
                limit =
                    limit.min(StorageError::DIAGNOSTIC_BYTES.saturating_sub(storage.bytes.get()));
            }
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
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
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
        let mut has_provider_diagnostic = false;
        loop {
            self.limit.set(KEY_BYTES);
            let Some(key) = map.next_key_seed(Key)? else {
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
pub(crate) fn preflight_checkpoint(reader: impl Read) -> Result<(), StorageError> {
    preflight_shape(reader, Shape::Checkpoint)
}

pub(super) fn preflight_semantic_batch(reader: impl Read) -> Result<(), StorageError> {
    preflight_shape(reader, Shape::SemanticBatch)
}

fn preflight_shape(reader: impl Read, shape: Shape) -> Result<(), StorageError> {
    let limit = Rc::new(Cell::new(KEY_BYTES));
    let reader = TokenReader::new(reader, limit.clone());
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    Seed {
        shape,
        limit,
        change: matches!(shape, Shape::Semantic | Shape::SemanticBatch).then(Rc::default),
        error: matches!(shape, Shape::Error | Shape::StorageError).then(Rc::default),
        recipe: None,
        error_depth: usize::from(matches!(shape, Shape::Error | Shape::StorageError)),
        storage: matches!(shape, Shape::StorageError).then(Rc::default),
        storage_node: matches!(shape, Shape::StorageError),
    }
    .deserialize(&mut deserializer)
    .map_err(|error| StorageError::Corrupt(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| StorageError::Corrupt(error.to_string()))
}

#[cfg(test)]
mod storage_tree_tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            agents::AgentError,
            sessions::{QueueHistoryRecord, SessionSnapshot},
        },
        infrastructure::session_storage::snapshot::errors::{SavedError, StorageFailure},
    };

    #[test]
    fn queue_history_preflight_consumes_semantic_structural_bound_in_either_order() {
        let maximum = QueueHistoryRecord::maximum_entries(SessionSnapshot::MAX_INVOCATIONS);
        for count in [maximum, maximum + 1] {
            let queue = std::iter::repeat_n(
                r#"{"mutation":"Restored","actor":null,"scheduling_length":null}"#,
                count,
            )
            .collect::<Vec<_>>()
            .join(",");
            for queue_first in [true, false] {
                let text = if queue_first {
                    format!(r#"{{"queue_history":[{queue}],"invocations":[]}}"#)
                } else {
                    format!(r#"{{"invocations":[],"queue_history":[{queue}]}}"#)
                };
                // This non-retaining bound uses the maximum possible invocation
                // count. The full validator still owns the exact relative bound.
                assert_eq!(
                    preflight_shape(text.as_bytes(), Shape::Snapshot).is_ok(),
                    count == maximum
                );
            }
        }
    }

    fn decode(text: &str) -> Result<StorageError, StorageError> {
        preflight_shape(text.as_bytes(), Shape::StorageError)?;
        let saved: StorageFailure =
            serde_json::from_str(text).map_err(|error| StorageError::Corrupt(error.to_string()))?;
        saved.try_into()
    }
    #[test]
    fn storage_diagnostic_budget_follows_its_retained_role() {
        let diagnostic = "x".repeat(StorageError::DIAGNOSTIC_BYTES + 904);
        let plain = serde_json::json!({"Storage": {"Io": diagnostic}}).to_string();
        preflight_shape(plain.as_bytes(), Shape::Error).unwrap();
        let saved: SavedError = serde_json::from_str(&plain).unwrap();
        let restored: AgentError = saved.try_into().unwrap();
        restored.validate_retained_size().unwrap();
        assert!(
            matches!(restored, AgentError::Storage(StorageError::Io(text)) if text == diagnostic)
        );
        let exact_ack =
            serde_json::json!({"Io": "x".repeat(StorageError::DIAGNOSTIC_BYTES)}).to_string();
        preflight_shape(exact_ack.as_bytes(), Shape::StorageError).unwrap();
        let ack = serde_json::json!({"Io": diagnostic}).to_string();
        assert!(preflight_shape(ack.as_bytes(), Shape::StorageError).is_err());
        let aggregate = serde_json::json!({"Storage": {"ShutdownFailures": {
            "read": {"Io": diagnostic}, "runtime": "Unresolved"
        }}})
        .to_string();
        assert!(preflight_shape(aggregate.as_bytes(), Shape::Error).is_err());
        let nested = serde_json::json!({"Storage": {"ShutdownFailures": {
            "read": {"Io": "x".repeat(StorageError::DIAGNOSTIC_BYTES)},
            "runtime": {"ShutdownFailures": {"read": {"Io": "y"}, "runtime": "Unresolved"}}
        }}})
        .to_string();
        assert!(preflight_shape(nested.as_bytes(), Shape::Error).is_err());
    }
    #[test]
    fn typed_shutdown_codec_refuses_hostile_trees_before_allocation() {
        let nested = r#"{"ShutdownFailures":{"read":{"ShutdownFailures":{"read":"ReadWorkerPanicked","runtime":"Unresolved"}},"runtime":{"Io":"cleanup"}}}"#;
        let restored = decode(nested).unwrap();
        let encoded = serde_json::to_string(&StorageFailure::from(restored.clone())).unwrap();
        assert_eq!(decode(&encoded).unwrap(), restored);
        for invalid in [
            r#"{"ShutdownFailures":{"read":"panic diagnostic","runtime":"Unresolved"}}"#,
            r#"{"ShutdownFailures":{"read":"ReadWorkerPanicked"}}"#,
            r#"{"ShutdownFailures":{"read":"ReadWorkerPanicked","runtime":"Unresolved","extra":true}}"#,
        ] {
            assert!(decode(invalid).is_err());
        }
        let exact = format!(
            r#"{{"ShutdownFailures":{{"read":{{"Io":"{}"}},"runtime":"Unresolved"}}}}"#,
            "x".repeat(StorageError::DIAGNOSTIC_BYTES)
        );
        assert!(decode(&exact).is_ok());
        let over = format!(
            r#"{{"ShutdownFailures":{{"read":{{"Io":"{}"}},"runtime":{{"Corrupt":"y"}}}}}}"#,
            "x".repeat(StorageError::DIAGNOSTIC_BYTES)
        );
        assert!(preflight_shape(over.as_bytes(), Shape::StorageError).is_err());
        assert!(decode(&over).is_err());
        fn binary(level: usize) -> String {
            if level == 0 {
                "\"Closed\"".into()
            } else {
                let child = binary(level - 1);
                format!(r#"{{"ShutdownFailures":{{"read":{child},"runtime":{child}}}}}"#)
            }
        }
        let tree = binary(6);
        let exact_agent = format!(
            r#"{{"ExecutionObservation":{{"error":{{"Storage":{tree}}},"execution_result":null}}}}"#
        );
        assert!(preflight_shape(exact_agent.as_bytes(), Shape::Error).is_ok());
        let over_agent = format!(
            r#"{{"ExecutionObservation":{{"error":{exact_agent},"execution_result":null}}}}"#
        );
        assert!(preflight_shape(over_agent.as_bytes(), Shape::Error).is_err());
        let over_storage =
            format!(r#"{{"ShutdownFailures":{{"read":{tree},"runtime":"Closed"}}}}"#);
        assert!(preflight_shape(over_storage.as_bytes(), Shape::StorageError).is_err());
        let mut deep = "\"Closed\"".to_owned();
        for _ in 1..DiagnosticTreeLimits::DEPTH {
            deep = format!(r#"{{"ShutdownFailures":{{"read":{deep},"runtime":"Unresolved"}}}}"#);
        }
        assert!(decode(&deep).is_ok());
        deep = format!(r#"{{"ShutdownFailures":{{"read":{deep},"runtime":"Unresolved"}}}}"#);
        assert!(preflight_shape(deep.as_bytes(), Shape::StorageError).is_err());
        assert!(decode(&deep).is_err());
        for _ in 0..10_000 {
            deep = format!(r#"{{"ShutdownFailures":{{"read":{deep},"runtime":"Unresolved"}}}}"#);
        }
        assert!(preflight_shape(deep.as_bytes(), Shape::StorageError).is_err());
        assert!(decode(&deep).is_err());
    }
}
