//! Notes as stored: each field as text or a number, read back through the
//! same value rules that built it.
//!
//! ```text
//! NoteRecord ──encode──▶ WireNote ──decode──▶ NoteRecord
//! ```
//!
//! One shape serves the semantic stream and the checkpoint. A checkpoint
//! without notes writes none.
use super::{initiation::Initiator, permissions::Actor, tools::corrupt};
use crate::application::agent_execution::sessions::{
    NoteDelivery, NoteId, NoteRecord, NoteText, StorageError,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireNote {
    id: String,
    text: String,
    delivery: Delivery,
    actor: Actor,
    author: Initiator,
    accepted_at_ms: u64,
}
#[derive(Serialize, Deserialize)]
enum Delivery {
    RecordOnly,
}
impl From<&NoteRecord> for WireNote {
    fn from(value: &NoteRecord) -> Self {
        Self {
            id: value.id.as_str().into(),
            text: value.text.as_str().into(),
            delivery: match value.delivery {
                NoteDelivery::RecordOnly => Delivery::RecordOnly,
            },
            actor: (&value.actor).into(),
            author: (&value.author).into(),
            accepted_at_ms: value.accepted_at_ms,
        }
    }
}
impl WireNote {
    pub(super) fn decode(self) -> Result<NoteRecord, StorageError> {
        Ok(NoteRecord {
            id: NoteId::new(self.id).map_err(corrupt)?,
            text: NoteText::new(self.text).map_err(corrupt)?,
            delivery: match self.delivery {
                Delivery::RecordOnly => NoteDelivery::RecordOnly,
            },
            actor: self.actor.decode()?,
            author: self.author.decode()?,
            accepted_at_ms: self.accepted_at_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{
        permissions::ActionContext,
        sessions::{ProviderContext, SessionChange, SessionSnapshot},
    };
    use crate::domain::agent_execution::executions::{
        GrantInitiator, Initiator as DomainInitiator,
    };
    use crate::infrastructure::session_storage::snapshot::{
        checkpoint::{history_fixture, Snapshot, SnapshotRef},
        decode::preflight_checkpoint,
        semantic::{decode_batch, encode_batch},
    };

    fn note(id: &str, request: &str, author: DomainInitiator) -> NoteRecord {
        NoteRecord {
            id: NoteId::new(id).unwrap(),
            text: NoteText::new("the parser review is ready").unwrap(),
            delivery: NoteDelivery::RecordOnly,
            actor: ActionContext::new("person", "phone", request).unwrap(),
            author,
            accepted_at_ms: 1_700_000_000_000,
        }
    }
    fn grant() -> DomainInitiator {
        DomainInitiator::Grant(GrantInitiator::new("receiver", 42).unwrap())
    }
    fn with_notes(notes: Vec<NoteRecord>) -> SessionSnapshot {
        let mut snapshot = history_fixture(1);
        snapshot.notes = notes;
        snapshot
    }
    fn checkpoint(snapshot: &SessionSnapshot) -> String {
        serde_json::to_string(&SnapshotRef(snapshot)).unwrap()
    }
    fn decode(text: &str) -> Result<SessionSnapshot, StorageError> {
        preflight_checkpoint(
            format!(
                r#"{{"schemaVersion":{},"snapshot":{text}}}"#,
                StorageError::SCHEMA_VERSION,
            )
            .as_bytes(),
        )?;
        serde_json::from_str::<Snapshot>(text)
            .map_err(corrupt)?
            .decode()
    }
    fn field(record: &NoteRecord, key: &str, value: serde_json::Value) -> String {
        let mut wire = serde_json::to_value(WireNote::from(record)).unwrap();
        wire[key] = value;
        serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [{ "Note": wire }]
        })
        .to_string()
    }

    /// Row J16: a note keeps its author, owner or grant, through the stream.
    #[test]
    fn a_note_round_trips_through_the_semantic_stream() {
        for record in [
            note("a", "one", DomainInitiator::Owner),
            note("b", "two", grant()),
        ] {
            let encoded = encode_batch(&[SessionChange::Note(record.clone())]).unwrap();
            let decoded = decode_batch(&encoded, &ProviderContext::Absent).unwrap();
            assert_eq!(decoded, vec![SessionChange::Note(record)]);
        }
    }

    #[test]
    fn a_stored_note_value_its_rule_refuses_is_corrupt() {
        let record = note("a", "one", grant());
        for (key, value) in [
            ("id", serde_json::json!(" ")),
            ("text", serde_json::json!("")),
            ("delivery", serde_json::json!("NextTurn")),
            (
                "actor",
                serde_json::json!({"principal_id": "", "surface_id": "s", "request_id": "r"}),
            ),
            (
                "author",
                serde_json::json!({"Grant": {"grant": " ", "policy_revision": 1}}),
            ),
            ("accepted_at_ms", serde_json::json!(-1)),
        ] {
            let batch = field(&record, key, value);
            assert!(
                matches!(
                    decode_batch(batch.as_bytes(), &ProviderContext::Absent),
                    Err(StorageError::Corrupt(_))
                ),
                "{key}"
            );
        }
    }

    #[test]
    fn note_text_past_its_bound_is_refused_before_it_is_read() {
        let record = note("a", "one", DomainInitiator::Owner);
        for (key, length) in [("id", NoteId::MAX_BYTES), ("text", NoteText::MAX_BYTES)] {
            let batch = field(&record, key, serde_json::json!("a".repeat(length + 1)));
            let refused = decode_batch(batch.as_bytes(), &ProviderContext::Absent).unwrap_err();
            assert!(
                matches!(&refused, StorageError::Corrupt(message) if message.contains("decoded string limit")),
                "{key}: {refused:?}"
            );
        }
        let batch = field(&record, "extra", serde_json::json!(1));
        assert!(matches!(
            decode_batch(batch.as_bytes(), &ProviderContext::Absent),
            Err(StorageError::Corrupt(message)) if message.contains("unknown field for its schema")
        ));
    }

    #[test]
    fn a_checkpoint_writes_no_notes_until_one_is_left_then_keeps_them_in_order() {
        let snapshot = history_fixture(1);
        let text = checkpoint(&snapshot);
        assert!(!text.contains("notes"));
        assert_eq!(decode(&text).unwrap().notes, Vec::new());
        let snapshot = with_notes(vec![
            note("a", "one", DomainInitiator::Owner),
            note("b", "two", grant()),
        ]);
        assert_eq!(decode(&checkpoint(&snapshot)).unwrap(), snapshot);
    }

    #[test]
    fn a_checkpoint_past_the_bound_or_repeating_a_request_is_refused() {
        let over: Vec<_> = (0..=NoteRecord::MAX_PER_CONVERSATION)
            .map(|index| note(&format!("id-{index}"), &format!("r-{index}"), grant()))
            .collect();
        assert!(matches!(
            decode(&checkpoint(&with_notes(over))),
            Err(StorageError::Corrupt(message)) if message.contains("collection exceeds decoding limit")
        ));
        let repeated = with_notes(vec![
            note("a", "one", DomainInitiator::Owner),
            note("b", "one", DomainInitiator::Owner),
        ]);
        assert!(matches!(
            decode(&checkpoint(&repeated)),
            Err(StorageError::Corrupt(_))
        ));
    }
}
