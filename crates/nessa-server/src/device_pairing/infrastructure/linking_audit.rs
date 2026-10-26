//! Durable evidence of each `pairing.setLinking` and `pairing.setTunnel`
//! change, in
//! `<namespace>/conversations/audit/native-linking`: one private JSON file per
//! record, named by its change and phase, published once and synced, as the
//! MCP server settings' audit beside it is. The requested record and its
//! outcome share the change's id; between changes the only order is
//! `observedAtMs`, on the wall clock.
use crate::device_pairing::application::{
    LinkingApply, LinkingAudit, LinkingAuditPhase, LinkingAuditRecord, LinkingAuditUnavailable,
    LinkingError, LinkingRestart, LinkingSubject,
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc};

/// The records, in their directory, stamped with when each was observed.
pub struct DurableLinkingAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableLinkingAudit {
    /// The audit in `directory`, created private by its first record: a
    /// gateway that never changes linking leaves no directory behind.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { directory, clock }
    }
}

fn stored(record: &LinkingAuditRecord) -> Value {
    let (phase, outcome) = match record.phase {
        LinkingAuditPhase::Requested => ("requested", Value::Null),
        LinkingAuditPhase::Applied {
            applies,
            written,
            moved,
        } => (
            "outcome",
            json!({
                "outcome": "applied",
                "applies": match applies {
                    LinkingApply::Unchanged => "unchanged",
                    LinkingApply::Restarting => "restarting",
                },
                "written": written,
                "listenMoved": moved.map(|moved| json!({
                    "from": moved.from.to_string(),
                    "to": moved.to.to_string(),
                })),
            }),
        ),
        LinkingAuditPhase::Restart(end) => (
            "restart",
            json!({
                "restart": match end {
                    LinkingRestart::Exec => "exec",
                    LinkingRestart::ExecFailed => "exec_failed",
                    LinkingRestart::Cancelled => "cancelled",
                },
            }),
        ),
        LinkingAuditPhase::Refused(reason) => (
            "outcome",
            json!({
                "outcome": "refused",
                "reason": match reason {
                    LinkingError::Busy => "busy",
                    LinkingError::AddressInUse => "address_in_use",
                    LinkingError::Unavailable => "unavailable",
                    LinkingError::AuditUnavailable => "audit_unavailable",
                    LinkingError::LinkingOff => "linking_off",
                    LinkingError::TunnelIncomplete => "tunnel_incomplete",
                },
            }),
        ),
    };
    let (kind, mode) = match record.subject {
        LinkingSubject::Linking => ("native_linking", Value::Null),
        LinkingSubject::Tunnel { mode } => (
            "native_tunnel",
            mode.map_or(Value::Null, |mode| json!(mode.as_str())),
        ),
    };
    let mut value = json!({
        "kind": kind,
        "operationId": record.operation_id,
        "phase": phase,
        "enabled": record.enabled,
        "running": record.running,
        "outcome": outcome,
        "cause": "caller_requested",
        "initiator": {
            "kind": "caller",
            "organizationId": record.caller.organization_id,
            "principalId": record.caller.principal_id,
            "credentialId": record.caller.credential_id,
        },
    });
    if let (LinkingSubject::Tunnel { .. }, Value::Object(fields)) = (record.subject, &mut value) {
        fields.insert("mode".into(), mode);
    }
    value
}

impl LinkingAudit for DurableLinkingAudit {
    fn record(&self, record: &LinkingAuditRecord) -> Result<(), LinkingAuditUnavailable> {
        let phase = match record.phase {
            LinkingAuditPhase::Requested => "requested",
            LinkingAuditPhase::Applied { .. } | LinkingAuditPhase::Refused(_) => "outcome",
            LinkingAuditPhase::Restart(LinkingRestart::Exec) => "exec",
            LinkingAuditPhase::Restart(LinkingRestart::ExecFailed) => "exec-failed",
            LinkingAuditPhase::Restart(LinkingRestart::Cancelled) => "cancelled",
        };
        create_directory(&self.directory).map_err(|_| LinkingAuditUnavailable)?;
        let subject = match record.subject {
            LinkingSubject::Linking => "native-linking",
            LinkingSubject::Tunnel { .. } => "native-tunnel",
        };
        let id = format!("{subject}-{}-{phase}", record.operation_id);
        let mut value = stored(record);
        if let Value::Object(fields) = &mut value {
            fields.insert("recordId".into(), json!(id));
            fields.insert("observedAtMs".into(), json!(self.clock.unix_milliseconds()));
        }
        let mut file =
            PrivateTempFile::new_in(&self.directory).map_err(|_| LinkingAuditUnavailable)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| LinkingAuditUnavailable)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| LinkingAuditUnavailable)?;
        // One change, one record of each phase: a name taken is a fault.
        file.publish(&self.directory.join(format!("{id}.json")))
            .map_err(|_| LinkingAuditUnavailable)?;
        sync_directory(&self.directory).map_err(|_| LinkingAuditUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_pairing::application::{LinkingCaller, ListenMoved};

    struct Fixed;
    impl Clock for Fixed {
        fn unix_milliseconds(&self) -> u64 {
            7
        }
    }

    #[test]
    fn each_phase_is_one_private_file_naming_the_caller() {
        let directory = tempfile::tempdir().unwrap();
        let audit =
            DurableLinkingAudit::new(directory.path().join("native-linking"), Arc::new(Fixed));
        assert!(!directory.path().join("native-linking").exists());
        let mut record = LinkingAuditRecord {
            operation_id: "op".into(),
            caller: LinkingCaller {
                organization_id: "org".into(),
                principal_id: "owner".into(),
                credential_id: "panel".into(),
            },
            subject: LinkingSubject::Linking,
            enabled: true,
            running: false,
            phase: LinkingAuditPhase::Requested,
        };
        audit.record(&record).unwrap();
        record.phase = LinkingAuditPhase::Applied {
            applies: LinkingApply::Restarting,
            written: true,
            moved: Some(ListenMoved {
                from: "127.0.0.1:47650".parse().unwrap(),
                to: "0.0.0.0:47650".parse().unwrap(),
            }),
        };
        audit.record(&record).unwrap();
        assert_eq!(
            audit.record(&record),
            Err(LinkingAuditUnavailable),
            "one outcome per change"
        );
        let read = |name: &str| -> Value {
            serde_json::from_slice(
                &std::fs::read(directory.path().join("native-linking").join(name)).unwrap(),
            )
            .unwrap()
        };
        let requested = read("native-linking-op-requested.json");
        assert_eq!(requested["initiator"]["principalId"], "owner");
        assert_eq!(requested["enabled"], true);
        let outcome = read("native-linking-op-outcome.json");
        assert_eq!(outcome["outcome"]["applies"], "restarting");
        assert_eq!(outcome["observedAtMs"], 7);
        assert_eq!(outcome["outcome"]["written"], true);
        assert_eq!(outcome["outcome"]["listenMoved"]["from"], "127.0.0.1:47650");
        assert_eq!(outcome["outcome"]["listenMoved"]["to"], "0.0.0.0:47650");
        record.phase = LinkingAuditPhase::Restart(LinkingRestart::Exec);
        audit.record(&record).unwrap();
        assert_eq!(
            read("native-linking-op-exec.json")["outcome"]["restart"],
            "exec"
        );
    }

    #[test]
    fn a_tunnel_change_is_its_own_kind_with_its_mode() {
        use crate::tunnel::application::TunnelMode;
        let directory = tempfile::tempdir().unwrap();
        let audit = DurableLinkingAudit::new(directory.path().to_owned(), Arc::new(Fixed));
        let record = LinkingAuditRecord {
            operation_id: "op".into(),
            caller: LinkingCaller {
                organization_id: "org".into(),
                principal_id: "owner".into(),
                credential_id: "panel".into(),
            },
            subject: LinkingSubject::Tunnel {
                mode: Some(TunnelMode::Named),
            },
            enabled: true,
            running: false,
            phase: LinkingAuditPhase::Refused(LinkingError::TunnelIncomplete),
        };
        audit.record(&record).unwrap();
        let outcome: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("native-tunnel-op-outcome.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcome["kind"], "native_tunnel");
        assert_eq!(outcome["mode"], "named");
        assert_eq!(outcome["outcome"]["reason"], "tunnel_incomplete");
    }
}
