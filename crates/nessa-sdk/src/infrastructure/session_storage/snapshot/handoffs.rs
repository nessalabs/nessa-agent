//! Handoffs as stored: each field as text or a number, read back through the
//! same value rules that built it.
//!
//! ```text
//! HandoffRecord ──encode──▶ WireHandoff ──decode──▶ HandoffRecord
//! ```
//!
//! One shape serves the semantic stream and the checkpoint. A checkpoint
//! without handoffs writes none.
use super::{agent_source::WireAgentSource, permissions::Actor, records::Provider, tools::corrupt};
use crate::application::agent_execution::sessions::{
    HandoffId, HandoffKind, HandoffRecord, StorageError,
};
use crate::domain::agent_execution::{
    executions::ExecutionId,
    prompts::{CarriedHistory, CarriedTurn},
    sessions::SessionId,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireHandoff {
    id: String,
    kind: WireKind,
    actor: Actor,
    at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_execution: Option<String>,
}
#[derive(Serialize, Deserialize)]
enum WireKind {
    Switch {
        before: Provider,
        after: Provider,
    },
    Fork {
        source: String,
        history: WireHistory,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireHistory {
    turns: Vec<WireTurn>,
    omitted: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTurn {
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<WireAgentSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    said: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply: Option<String>,
}
impl From<&HandoffRecord> for WireHandoff {
    fn from(value: &HandoffRecord) -> Self {
        Self {
            id: value.id.as_str().into(),
            kind: match &value.kind {
                HandoffKind::Switch { before, after } => WireKind::Switch {
                    before: before.into(),
                    after: after.into(),
                },
                HandoffKind::Fork { source, history } => WireKind::Fork {
                    source: source.as_str().into(),
                    history: WireHistory {
                        turns: history
                            .turns()
                            .iter()
                            .map(|turn| WireTurn {
                                agent: turn.agent_source().map(Into::into),
                                said: turn.said().map(Into::into),
                                reply: turn.reply().map(Into::into),
                            })
                            .collect(),
                        omitted: history.omitted(),
                    },
                },
            },
            actor: (&value.actor).into(),
            at_ms: value.at_ms,
            after_execution: value.after_execution.as_ref().map(|id| id.as_str().into()),
        }
    }
}
impl WireHandoff {
    pub(super) fn decode(self) -> Result<HandoffRecord, StorageError> {
        Ok(HandoffRecord {
            id: HandoffId::new(self.id)?,
            kind: match self.kind {
                WireKind::Switch { before, after } => HandoffKind::Switch {
                    before: before.decode()?,
                    after: after.decode()?,
                },
                WireKind::Fork { source, history } => HandoffKind::Fork {
                    source: SessionId::new(source).map_err(corrupt)?,
                    history: CarriedHistory::new(
                        history
                            .turns
                            .into_iter()
                            .map(|turn| {
                                CarriedTurn::new(
                                    turn.said.as_deref(),
                                    turn.reply.as_deref(),
                                    turn.agent.map(WireAgentSource::decode).transpose()?,
                                )
                                .map_err(corrupt)
                            })
                            .collect::<Result<_, _>>()?,
                        history.omitted,
                    )
                    .map_err(corrupt)?,
                },
            },
            actor: self.actor.decode()?,
            at_ms: self.at_ms,
            after_execution: self
                .after_execution
                .map(ExecutionId::new)
                .transpose()
                .map_err(corrupt)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{
        permissions::ActionContext,
        providers::ProviderIdentity,
        sessions::{ProviderContext, SessionChange, SessionSnapshot},
    };
    use crate::infrastructure::session_storage::snapshot::{
        checkpoint::{history_fixture, Snapshot, SnapshotRef},
        decode::preflight_checkpoint,
        semantic::{decode_batch, encode_batch},
    };

    fn actor(request: &str) -> ActionContext {
        ActionContext::new("person", "desktop", request).unwrap()
    }
    fn switch(request: &str, after: &str) -> HandoffRecord {
        HandoffRecord {
            id: HandoffId::of_request(&actor(request)),
            kind: HandoffKind::Switch {
                before: ProviderIdentity::new("claude", "opus", "workspace").unwrap(),
                after: ProviderIdentity::new("codex", "gpt", "workspace").unwrap(),
            },
            actor: actor(request),
            at_ms: 1_700_000_000_000,
            after_execution: Some(ExecutionId::new(after).unwrap()),
        }
    }
    fn fork(request: &str) -> HandoffRecord {
        HandoffRecord {
            id: HandoffId::of_request(&actor(request)),
            kind: HandoffKind::Fork {
                source: SessionId::new("source").unwrap(),
                history: CarriedHistory::new(
                    vec![
                        CarriedTurn::new(
                            Some("plot May"),
                            Some("done"),
                            Some(
                                crate::domain::agent_execution::executions::AgentSource::new(
                                    "8b547a4b-6974-49a4-b878-d13a9c031f16",
                                    ExecutionId::new("source-turn").unwrap(),
                                )
                                .unwrap(),
                            ),
                        )
                        .unwrap(),
                        CarriedTurn::new(None, Some("only a reply"), None).unwrap(),
                    ],
                    4,
                )
                .unwrap(),
            },
            actor: actor(request),
            at_ms: 1_700_000_000_000,
            after_execution: None,
        }
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
    fn batch(wire: serde_json::Value) -> String {
        serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [{ "Handoff": wire }]
        })
        .to_string()
    }

    #[test]
    fn a_handoff_round_trips_through_the_semantic_stream() {
        for record in [switch("one", "input-0"), fork("two")] {
            let encoded = encode_batch(&[SessionChange::Handoff(record.clone())]).unwrap();
            let decoded = decode_batch(&encoded, &ProviderContext::Absent).unwrap();
            assert_eq!(decoded, vec![SessionChange::Handoff(record)]);
        }
    }

    /// A checkpoint writes no handoffs until one is recorded, then keeps
    /// them and the turn that took each.
    #[test]
    fn a_checkpoint_keeps_its_handoffs_and_the_turns_that_took_them() {
        let mut snapshot = history_fixture(2);
        let text = checkpoint(&snapshot);
        assert!(!text.contains("handoff"));
        assert_eq!(decode(&text).unwrap().handoffs, Vec::new());
        snapshot.handoffs = vec![switch("one", "input-0")];
        snapshot.invocations[1].handoff = Some(snapshot.handoffs[0].id.clone());
        assert_eq!(decode(&checkpoint(&snapshot)).unwrap(), snapshot);
        let mut forked = history_fixture(0);
        forked.handoffs = vec![fork("two")];
        assert_eq!(decode(&checkpoint(&forked)).unwrap(), forked);
    }

    #[test]
    fn a_stored_handoff_its_rules_refuse_is_corrupt() {
        let wire = serde_json::to_value(WireHandoff::from(&fork("one"))).unwrap();
        let mut cases = Vec::new();
        for (key, value) in [
            ("id", serde_json::json!(" ")),
            ("at_ms", serde_json::json!(-1)),
            ("after_execution", serde_json::json!("")),
            ("extra", serde_json::json!(1)),
        ] {
            let mut changed = wire.clone();
            changed[key] = value;
            cases.push((key, changed));
        }
        for agent in [
            serde_json::json!({"conversation":"not-a-uuid", "execution":"source-turn"}),
            serde_json::json!({"conversation":"8b547a4b-6974-49a4-b878-d13a9c031f16", "execution":""}),
            serde_json::json!({"conversation":"8b547a4b-6974-49a4-b878-d13a9c031f16", "execution":"x".repeat(ExecutionId::MAX_BYTES+1)}),
        ] {
            let mut changed = wire.clone();
            changed["kind"]["Fork"]["history"]["turns"][0]["agent"] = agent;
            cases.push(("invalid copied agent", changed));
        }
        let mut blank = wire.clone();
        blank["kind"]["Fork"]["history"]["turns"] = serde_json::json!([{ "said": " " }]);
        cases.push(("blank turn", blank));
        let mut long = wire.clone();
        long["kind"]["Fork"]["history"]["turns"] =
            serde_json::json!([{ "said": "a".repeat(CarriedTurn::MAX_PART_BYTES + 1) }]);
        cases.push(("long turn", long));
        let mut odd = wire.clone();
        odd["kind"]["Fork"]["history"]["turns"] = serde_json::json!([{ "other": "a" }]);
        cases.push(("unknown turn field", odd));
        let mut over = wire.clone();
        over["kind"]["Fork"]["history"]["turns"] = serde_json::json!(vec![
            serde_json::json!({ "said": "a" });
            CarriedHistory::MAX_TURNS
                + 1
        ]);
        cases.push(("too many turns", over));
        for (case, wire) in cases {
            let refused = decode_batch(batch(wire).as_bytes(), &ProviderContext::Absent);
            assert!(matches!(refused, Err(StorageError::Corrupt(_))), "{case}");
        }
    }
}
