//! Durable evidence for a provider approval-preset application and recovery.
use crate::conversation::application::{
    ConversationError, ConversationFuture, ConversationModeApplication, ConversationModeAudit,
    ConversationModeAuditPhase, ConversationModeRequest,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct DurableConversationModeAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl DurableConversationModeAudit {
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, ConversationError> {
        create_directory(&directory).map_err(|_| ConversationError::Audit)?;
        Ok(Self { directory, clock })
    }
}
impl ConversationModeAudit for DurableConversationModeAudit {
    fn record(
        &self,
        request: ConversationModeRequest,
        phase: ConversationModeAuditPhase,
    ) -> ConversationFuture<'_, ()> {
        let phase_name = match phase {
            ConversationModeAuditPhase::AdmissionRefused => "admission_refused",
            ConversationModeAuditPhase::Application => "application",
            ConversationModeAuditPhase::RecoveryRestored => "recovery_restored",
        };
        let application = request.application.map(|application| match application {
            ConversationModeApplication::Deferred => "deferred",
            ConversationModeApplication::Applied => "applied",
            ConversationModeApplication::Refused => "refused",
            ConversationModeApplication::Uncertain => "uncertain",
        });
        let correlation = format!(
            "{}:{}:{phase_name}",
            request.conversation_id, request.request_id
        );
        let digest = Sha256::digest(correlation.as_bytes());
        let id = format!("conversation-mode-{digest:x}");
        let value = json!({
            "recordId": id,
            "kind": "conversation_approval_mode",
            "target": {
                "conversationId": request.conversation_id.to_string(),
                "organizationId": request.organization_id.as_str(),
                "principalId": request.initiator_principal_id.as_str(),
            },
            "transition": {
                "before": request.prior.as_str(),
                "requested": request.requested.as_str(),
                "application": application,
                "phase": phase_name,
                "committedAfterRecovery": if phase == ConversationModeAuditPhase::RecoveryRestored {
                    Some(request.prior.as_str())
                } else {
                    None
                },
            },
            "cause": match phase {
                ConversationModeAuditPhase::AdmissionRefused => "turn_running",
                ConversationModeAuditPhase::Application => "caller_requested",
                ConversationModeAuditPhase::RecoveryRestored => "unfinished_change_recovery",
            },
            "initiator": if phase != ConversationModeAuditPhase::RecoveryRestored {
                json!({"kind":"person","principalId":request.initiator_principal_id.as_str(),
                    "surfaceId":request.initiator_surface_id})
            } else {
                json!({"kind":"system","originalPrincipalId":request.initiator_principal_id.as_str(),
                    "originalSurfaceId":request.initiator_surface_id})
            },
            "correlationId": request.request_id,
            "requestedAtMs": request.requested_at_ms,
            // This is when the adapter observed the phase for durable delivery,
            // not the unknown time of an external provider effect.
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let directory = self.directory.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let destination = directory.join(format!("{id}.json"));
                if agrees(&destination, &value)? {
                    return sync_directory(&directory).map_err(|_| ConversationError::Audit);
                }
                let mut file =
                    PrivateTempFile::new_in(&directory).map_err(|_| ConversationError::Audit)?;
                serde_json::to_writer(file.as_file_mut(), &value)
                    .map_err(|_| ConversationError::Audit)?;
                file.as_file_mut()
                    .write_all(b"\n")
                    .and_then(|_| file.as_file().sync_all())
                    .map_err(|_| ConversationError::Audit)?;
                if let Err(error) = file.publish(&destination) {
                    if error.kind() != ErrorKind::AlreadyExists || !agrees(&destination, &value)? {
                        return Err(ConversationError::Audit);
                    }
                }
                sync_directory(&directory).map_err(|_| ConversationError::Audit)
            })
            .await
            .map_err(|_| ConversationError::Audit)?
        })
    }
}
fn agrees(destination: &Path, expected: &Value) -> Result<bool, ConversationError> {
    let mut file = match open(destination, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ConversationError::Audit),
    };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|_| ConversationError::Audit)?;
    if bytes.len() > 16_384 {
        return Err(ConversationError::Audit);
    }
    let mut stored: Value = serde_json::from_slice(&bytes).map_err(|_| ConversationError::Audit)?;
    let mut expected = expected.clone();
    let _ = stored
        .as_object_mut()
        .and_then(|value| value.remove("requestedAtMs"));
    let _ = expected
        .as_object_mut()
        .and_then(|value| value.remove("requestedAtMs"));
    let _ = stored
        .as_object_mut()
        .and_then(|value| value.remove("observedAtMs"));
    let _ = expected
        .as_object_mut()
        .and_then(|value| value.remove("observedAtMs"));
    if stored != expected {
        return Err(ConversationError::Audit);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::application::ConversationModeRequestState;
    use crate::conversation::domain::{ConversationApprovalMode, ConversationId};
    use nessa_auth::domain::{OrganizationId, PrincipalId};
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestClock(AtomicU64);
    impl Clock for TestClock {
        fn unix_milliseconds(&self) -> u64 {
            self.0.fetch_add(1, Ordering::SeqCst)
        }
    }

    fn request() -> ConversationModeRequest {
        ConversationModeRequest {
            conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000231").unwrap(),
            organization_id: OrganizationId::new("org").unwrap(),
            request_id: "mode-auto".into(),
            initiator_principal_id: PrincipalId::new("person").unwrap(),
            initiator_surface_id: "panel".into(),
            prior: ConversationApprovalMode::Ask,
            requested: ConversationApprovalMode::Auto,
            state: ConversationModeRequestState::Pending,
            application: Some(ConversationModeApplication::Applied),
            requested_at_ms: 231,
        }
    }

    #[tokio::test]
    async fn application_evidence_is_immutable_and_recovery_has_its_own_record() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurableConversationModeAudit::new(
            root.path().join("mode"),
            Arc::new(TestClock(AtomicU64::new(500))),
        )
        .unwrap();
        let first = request();
        audit
            .record(first.clone(), ConversationModeAuditPhase::Application)
            .await
            .unwrap();
        audit
            .record(first.clone(), ConversationModeAuditPhase::Application)
            .await
            .unwrap();
        audit
            .record(first.clone(), ConversationModeAuditPhase::RecoveryRestored)
            .await
            .unwrap();
        let records = std::fs::read_dir(root.path().join("mode"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice::<Value>(&std::fs::read(entry.unwrap().path()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|record| {
            record["target"]["organizationId"] == "org"
                && record["transition"]["before"] == "ask"
                && record["transition"]["requested"] == "auto"
                && record["transition"]["phase"] == "application"
                && record["initiator"]["principalId"] == "person"
                && record["requestedAtMs"] == 231
                && record["observedAtMs"] == 500
        }));
        assert!(records.iter().any(|record| {
            record["transition"]["phase"] == "recovery_restored"
                && record["transition"]["committedAfterRecovery"] == "ask"
                && record["initiator"]["originalPrincipalId"] == "person"
                && record["observedAtMs"] == 502
        }));
        assert!(matches!(
            audit
                .record(
                    ConversationModeRequest {
                        requested: ConversationApprovalMode::Full,
                        ..first
                    },
                    ConversationModeAuditPhase::Application,
                )
                .await,
            Err(ConversationError::Audit)
        ));
    }
}
