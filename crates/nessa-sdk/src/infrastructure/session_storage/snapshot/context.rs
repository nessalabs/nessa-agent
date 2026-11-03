//! One wire mapping for the retained native context and its admission boundary.
use super::tools::corrupt;
use crate::{
    application::agent_execution::sessions::{ProviderContext, StorageError},
    domain::agent_execution::sessions::ExecutionSessionId,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Context {
    id: String,
    admitted_before: usize,
}

pub(super) fn encode(context: &ProviderContext) -> Option<Context> {
    context.recorded().map(|id| Context {
        id: id.as_str().into(),
        admitted_before: context.admission_watermark().expect("recorded context"),
    })
}

pub(super) fn decode(context: Option<Context>) -> Result<ProviderContext, StorageError> {
    context.map_or(Ok(ProviderContext::Absent), |context| {
        Ok(ProviderContext::recorded_after(
            ExecutionSessionId::new(context.id).map_err(corrupt)?,
            context.admitted_before,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            agents::AgentError,
            sessions::{records::fold_changes, SessionChange},
        },
        infrastructure::session_storage::snapshot::{
            checkpoint::{history_fixture, Snapshot, SnapshotRef},
            decode::preflight_semantic_batch,
            semantic::{decode_batch, encode_batch},
        },
    };

    fn completed_history() -> crate::application::agent_execution::sessions::SessionSnapshot {
        let mut snapshot = history_fixture(2);
        for invocation in &mut snapshot.invocations {
            invocation.result = Some(Err(AgentError::InvalidInput("fixture refusal".into())));
        }
        snapshot.provider_context =
            ProviderContext::recorded_after(ExecutionSessionId::new("earlier-context").unwrap(), 0);
        snapshot
    }

    #[test]
    fn context_admission_survives_semantic_and_checkpoint_reopen() {
        let snapshot = completed_history();
        let after =
            ProviderContext::recorded_after(ExecutionSessionId::new("fresh-context").unwrap(), 2);
        let change = SessionChange::ProviderContext {
            before: snapshot.provider_context.clone(),
            after: after.clone(),
        };
        let bytes = encode_batch(std::slice::from_ref(&change)).unwrap();
        let decoded = decode_batch(&bytes, &snapshot.provider_context).unwrap();
        assert_eq!(decoded, vec![change]);
        let published = fold_changes(Some(&snapshot), &decoded).unwrap();
        assert_eq!(published.provider_context, after);
        assert!(!published.provider_context.has_admitted_work(2));
        let checkpoint = serde_json::to_vec(&SnapshotRef(&published)).unwrap();
        let saved: Snapshot = serde_json::from_slice(&checkpoint).unwrap();
        let restored = saved.decode().unwrap();
        assert_eq!(restored, published);
        assert!(!restored.provider_context.has_admitted_work(2));
        assert!(restored.provider_context.has_admitted_work(3));
    }

    #[test]
    fn context_admission_checkpoint_rejects_missing_or_contradictory_boundary() {
        let snapshot = completed_history();
        let original = serde_json::to_value(SnapshotRef(&snapshot)).unwrap();
        for invalid in [
            serde_json::json!({"id": "fresh-context"}),
            serde_json::json!({"id": "fresh-context", "admitted_before": 3}),
            serde_json::json!({"id": "fresh-context", "admitted_before": -1}),
            serde_json::json!("fresh-context"),
        ] {
            let mut checkpoint = original.clone();
            checkpoint["context"] = invalid;
            let restored = serde_json::from_value::<Snapshot>(checkpoint)
                .map_err(|_| ())
                .and_then(|saved| saved.decode().map_err(|_| ()));
            assert!(restored.is_err());
        }
    }

    #[test]
    fn context_identity_is_bounded_before_semantic_decode() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": StorageError::SCHEMA_VERSION,
            "changes": [{"ProviderContext": {
                "before": null,
                "after": {"id": "x".repeat(257), "admitted_before": 0}
            }}]
        }))
        .unwrap();
        assert!(preflight_semantic_batch(bytes.as_slice()).is_err());
    }
}
