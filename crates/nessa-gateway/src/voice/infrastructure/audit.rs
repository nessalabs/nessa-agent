//! Durable evidence of each voice key change and each call started, in
//! `<namespace>/conversations/audit/voice`: one private JSON file per record,
//! named by its operation and phase, published once and synced, as the
//! native-linking audit beside it is. Never the key or a client secret.
use std::{io::Write, path::PathBuf, sync::Arc};

use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};

use crate::voice::application::{
    AuditUnavailable, MintFailure, VoiceAudit, VoiceEvent, VoiceRecord,
};

pub struct DurableVoiceAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableVoiceAudit {
    /// The audit in `directory`, created private by its first record.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { directory, clock }
    }
}

/// The record's phase in its file name, and what it holds.
fn stored(record: &VoiceRecord) -> (&'static str, Value) {
    let (phase, kind, details) = match &record.event {
        VoiceEvent::KeySaveRequested { replacing } => (
            "requested",
            "voice_key_save",
            json!({ "replacing": replacing }),
        ),
        VoiceEvent::KeySaveFailed => ("failed", "voice_key_save", json!({})),
        VoiceEvent::KeySaved => ("saved", "voice_key_save", json!({})),
        VoiceEvent::KeyForgetRequested => ("requested", "voice_key_forget", json!({})),
        VoiceEvent::KeyForgetFailed => ("failed", "voice_key_forget", json!({})),
        VoiceEvent::KeyForgotten => ("forgotten", "voice_key_forget", json!({})),
        VoiceEvent::CallMinted { conversation_id } => (
            "minted",
            "voice_call",
            json!({ "conversationId": conversation_id }),
        ),
        VoiceEvent::CallRequested { conversation_id } => (
            "requested",
            "voice_call",
            json!({ "conversationId": conversation_id }),
        ),
        VoiceEvent::CallRefused {
            conversation_id,
            reason,
        } => (
            "refused",
            "voice_call",
            json!({
                "conversationId": conversation_id,
                "reason": match reason {
                    MintFailure::Rejected => "key_rejected",
                    MintFailure::Unreachable => "unreachable",
                },
            }),
        ),
    };
    (
        phase,
        json!({
            "kind": kind,
            "operationId": record.operation_id,
            "phase": phase,
            "details": details,
            "cause": "caller_requested",
            "initiator": {
                "kind": "caller",
                "organizationId": record.caller.organization_id,
                "principalId": record.caller.principal_id,
                "credentialId": record.caller.credential_id,
            },
        }),
    )
}

impl VoiceAudit for DurableVoiceAudit {
    fn record(&self, record: &VoiceRecord) -> Result<(), AuditUnavailable> {
        let (phase, mut value) = stored(record);
        create_directory(&self.directory).map_err(|_| AuditUnavailable)?;
        let id = format!("voice-{}-{phase}", record.operation_id);
        if let Value::Object(fields) = &mut value {
            fields.insert("recordId".into(), json!(id));
            fields.insert("observedAtMs".into(), json!(self.clock.unix_milliseconds()));
        }
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(|_| AuditUnavailable)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| AuditUnavailable)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| AuditUnavailable)?;
        // One operation, one record of each phase: a name taken is a fault.
        file.publish(&self.directory.join(format!("{id}.json")))
            .map_err(|_| AuditUnavailable)?;
        sync_directory(&self.directory).map_err(|_| AuditUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::application::VoiceCaller;

    struct Fixed;
    impl Clock for Fixed {
        fn unix_milliseconds(&self) -> u64 {
            7
        }
    }

    #[test]
    fn each_phase_is_one_file_naming_the_caller_and_never_a_secret() {
        let directory = tempfile::tempdir().unwrap();
        let audit = DurableVoiceAudit::new(directory.path().join("voice"), Arc::new(Fixed));
        assert!(!directory.path().join("voice").exists());
        let caller = VoiceCaller {
            organization_id: "org".into(),
            principal_id: "owner".into(),
            credential_id: "panel".into(),
        };
        let requested = VoiceRecord {
            operation_id: "op".into(),
            caller: caller.clone(),
            event: VoiceEvent::CallRequested {
                conversation_id: "c".into(),
            },
        };
        audit.record(&requested).unwrap();
        audit
            .record(&VoiceRecord {
                event: VoiceEvent::CallRefused {
                    conversation_id: "c".into(),
                    reason: MintFailure::Rejected,
                },
                ..requested.clone()
            })
            .unwrap();
        let read = |name: &str| -> Value {
            serde_json::from_slice(
                &std::fs::read(directory.path().join("voice").join(name)).unwrap(),
            )
            .unwrap()
        };
        let first = read("voice-op-requested.json");
        assert_eq!(first["kind"], "voice_call");
        assert_eq!(first["initiator"]["credentialId"], "panel");
        assert_eq!(first["observedAtMs"], 7);
        assert_eq!(
            read("voice-op-refused.json")["details"]["reason"],
            "key_rejected"
        );
        // A second record of the same phase is refused, not overwritten.
        assert_eq!(audit.record(&requested), Err(AuditUnavailable));
    }
}
