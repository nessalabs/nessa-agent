//! Artifact records as stored: each field as text or a number, read back
//! through the same value rules that built it.
//!
//! ```text
//! ArtifactRecord ──encode──▶ WireArtifact ──decode──▶ ArtifactRecord
//! ```
//!
//! One shape serves the semantic stream and the checkpoint. A checkpoint
//! without artifacts writes none, so it reads back exactly as one written
//! before artifacts existed.
use super::{permissions::Actor, tools::corrupt};
use crate::{
    application::agent_execution::sessions::{
        ArtifactName, ArtifactRecord, PublishedFile, StorageError,
    },
    domain::{
        agent_execution::{executions::ExecutionId, leases::LeaseId},
        common::value_objects::{MediaType, Sha256Digest},
    },
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireArtifact {
    lease: String,
    turn: Option<String>,
    name: String,
    digest: String,
    media_type: String,
    size: u64,
    actor: Actor,
}
impl From<&ArtifactRecord> for WireArtifact {
    fn from(value: &ArtifactRecord) -> Self {
        Self {
            lease: value.lease.as_str().into(),
            turn: value.turn.as_ref().map(|turn| turn.as_str().into()),
            name: value.name.as_str().into(),
            digest: value.file.digest().to_string(),
            media_type: value.file.media_type().as_str().into(),
            size: value.file.size(),
            actor: (&value.actor).into(),
        }
    }
}
impl WireArtifact {
    pub(super) fn decode(self) -> Result<ArtifactRecord, StorageError> {
        Ok(ArtifactRecord {
            lease: LeaseId::new(self.lease).map_err(corrupt)?,
            turn: self
                .turn
                .map(ExecutionId::new)
                .transpose()
                .map_err(corrupt)?,
            name: ArtifactName::new(self.name).map_err(corrupt)?,
            file: PublishedFile::new(
                Sha256Digest::parse(&self.digest).map_err(corrupt)?,
                MediaType::parse(&self.media_type).map_err(corrupt)?,
                self.size,
            )
            .map_err(corrupt)?,
            actor: self.actor.decode()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{
        permissions::ActionContext,
        sessions::{CurrentLease, LeaseRecord, ProviderContext, SessionChange, SessionSnapshot},
    };
    use crate::domain::agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseGrants, LeaseRevision, LeaseTerms,
        LeaseWork, SandboxProfile,
    };
    use crate::infrastructure::session_storage::snapshot::{
        checkpoint::{history_fixture, Snapshot, SnapshotRef},
        decode::preflight_checkpoint,
        semantic::{decode_batch, encode_batch},
    };

    const DIGEST: &str = "sha256:00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff";

    fn actor() -> ActionContext {
        ActionContext::new("person", "desktop", "send").unwrap()
    }
    fn issued() -> LeaseRecord {
        LeaseRecord::Issued {
            lease: LeaseId::new("lease-1").unwrap(),
            revision: LeaseRevision::FIRST,
            terms: LeaseTerms {
                environment: EnvironmentRef::Here,
                work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
                sandbox: SandboxProfile::HarnessDefault,
                grants: LeaseGrants::Opening,
                deadline: LeaseDeadline::UntilEnded,
            },
            actor: actor(),
        }
    }
    fn artifact(name: &str, turn: Option<&str>) -> ArtifactRecord {
        ArtifactRecord {
            lease: LeaseId::new("lease-1").unwrap(),
            turn: turn.map(|turn| ExecutionId::new(turn).unwrap()),
            name: ArtifactName::new(name).unwrap(),
            file: PublishedFile::new(
                Sha256Digest::parse(DIGEST).unwrap(),
                MediaType::parse("text/plain").unwrap(),
                12,
            )
            .unwrap(),
            actor: actor(),
        }
    }
    fn with_artifacts(artifacts: Vec<ArtifactRecord>) -> SessionSnapshot {
        let mut snapshot = history_fixture(1);
        snapshot.lease = Some(CurrentLease::apply(None, &issued()).unwrap());
        snapshot.artifacts = artifacts;
        snapshot
    }
    fn checkpoint(snapshot: &SessionSnapshot) -> String {
        serde_json::to_string(&SnapshotRef(snapshot)).unwrap()
    }
    fn decode(text: &str) -> Result<SessionSnapshot, StorageError> {
        preflight_checkpoint(format!(r#"{{"snapshot":{text}}}"#).as_bytes())?;
        serde_json::from_str::<Snapshot>(text)
            .map_err(corrupt)?
            .decode()
    }
    fn field(record: &ArtifactRecord, key: &str, value: serde_json::Value) -> String {
        let mut wire = serde_json::to_value(WireArtifact::from(record)).unwrap();
        wire[key] = value;
        serde_json::json!({ "changes": [{ "Artifact": wire }] }).to_string()
    }

    #[test]
    fn an_artifact_record_round_trips_through_the_semantic_stream() {
        for record in [
            artifact("report.pdf", Some("input-0")),
            artifact("é.txt", None),
        ] {
            let encoded = encode_batch(&[SessionChange::Artifact(record.clone())]).unwrap();
            let decoded = decode_batch(&encoded, &ProviderContext::Absent).unwrap();
            assert_eq!(decoded, vec![SessionChange::Artifact(record)]);
        }
    }

    #[test]
    fn a_stored_value_its_rule_refuses_is_corrupt() {
        let record = artifact("report.pdf", None);
        for (key, value) in [
            ("lease", serde_json::json!("not a lease")),
            ("turn", serde_json::json!("")),
            ("name", serde_json::json!("a/b")),
            ("digest", serde_json::json!("sha256:00")),
            ("media_type", serde_json::json!("Text/Plain")),
            ("size", serde_json::json!(0)),
            (
                "actor",
                serde_json::json!({"principal_id": "", "surface_id": "s", "request_id": "r"}),
            ),
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
    fn artifact_text_past_its_bound_is_refused_before_it_is_read() {
        let record = artifact("report.pdf", None);
        for (key, length) in [
            ("lease", LeaseId::MAX_BYTES),
            ("turn", ExecutionId::MAX_BYTES),
            ("name", ArtifactName::MAX_BYTES),
            ("digest", DIGEST.len()),
            ("media_type", MediaType::MAX_BYTES),
        ] {
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
    fn a_checkpoint_without_artifacts_is_written_and_read_as_before_artifacts_existed() {
        let snapshot = history_fixture(1);
        let text = checkpoint(&snapshot);
        assert!(!text.contains("artifacts"));
        assert_eq!(decode(&text).unwrap().artifacts, Vec::new());
    }

    #[test]
    fn a_checkpoint_keeps_its_artifacts_in_order() {
        let snapshot = with_artifacts(vec![
            artifact("first.txt", Some("input-0")),
            artifact("second.txt", None),
        ]);
        assert_eq!(decode(&checkpoint(&snapshot)).unwrap(), snapshot);
    }

    #[test]
    fn a_checkpoint_past_the_conversation_bound_is_refused_before_it_is_built() {
        let snapshot = with_artifacts(vec![
            artifact("a.txt", None);
            ArtifactRecord::MAX_PER_CONVERSATION + 1
        ]);
        assert!(matches!(
            decode(&checkpoint(&snapshot)),
            Err(StorageError::Corrupt(message)) if message.contains("collection exceeds decoding limit")
        ));
        let full = with_artifacts(vec![
            artifact("a.txt", None);
            ArtifactRecord::MAX_PER_CONVERSATION
        ]);
        assert_eq!(decode(&checkpoint(&full)).unwrap(), full);
    }

    #[test]
    fn a_checkpoint_artifact_naming_a_turn_never_accepted_is_corrupt() {
        let snapshot = with_artifacts(vec![artifact("a.txt", Some("never"))]);
        assert!(matches!(
            decode(&checkpoint(&snapshot)),
            Err(StorageError::Corrupt(_))
        ));
    }
}
