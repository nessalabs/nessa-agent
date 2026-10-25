//! Durable evidence of each GitHub sign-in and sign-out, in
//! `<namespace>/conversations/audit/github`: one private JSON file per record,
//! named by its operation and event, published once and synced, as the
//! native-linking audit beside it is. Never a token or a code: an account is
//! named by its id and login. Between operations the only order is
//! `observedAtMs`, on the wall clock.
use std::{io::Write, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};

use crate::github::application::{
    AuditUnavailable, GithubAudit, GithubAuditEvent, GithubAuditRecord, GithubCaller,
};
use crate::github::domain::{ConnectEnd, GithubAccount};

pub struct DurableGithubAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableGithubAudit {
    /// The audit in `directory`, created private by its first record.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { directory, clock }
    }

    fn write(&self, record: &GithubAuditRecord) -> Result<(), AuditUnavailable> {
        create_directory(&self.directory).map_err(|_| AuditUnavailable)?;
        let id = format!("github-{}-{}", record.operation_id, name(&record.event));
        let mut value = stored(record);
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
        // One operation, one record of each event: a name taken is a fault.
        file.publish(&self.directory.join(format!("{id}.json")))
            .map_err(|_| AuditUnavailable)?;
        sync_directory(&self.directory).map_err(|_| AuditUnavailable)
    }
}

#[async_trait]
impl GithubAudit for DurableGithubAudit {
    async fn record(&self, record: &GithubAuditRecord) -> Result<(), AuditUnavailable> {
        let audit = Self::new(self.directory.clone(), self.clock.clone());
        let record = record.clone();
        tokio::task::spawn_blocking(move || audit.write(&record))
            .await
            .map_err(|_| AuditUnavailable)?
    }
}

/// The event's part of a record's name: each is written once per operation.
fn name(event: &GithubAuditEvent) -> &'static str {
    match event {
        GithubAuditEvent::ConnectRequested => "connect-requested",
        GithubAuditEvent::Connected { .. } => "connected",
        GithubAuditEvent::ConnectEnded(_) => "connect-ended",
        GithubAuditEvent::ConnectCancelled { .. } => "connect-cancelled",
        GithubAuditEvent::DisconnectRequested { .. } => "disconnect-requested",
        GithubAuditEvent::Disconnected => "disconnected",
        GithubAuditEvent::DisconnectFailed => "disconnect-failed",
    }
}

fn caller(caller: &GithubCaller) -> Value {
    json!({
        "kind": "caller",
        "organizationId": caller.organization_id,
        "principalId": caller.principal_id,
        "credentialId": caller.credential_id,
    })
}

fn account(account: &GithubAccount) -> Value {
    json!({ "id": account.id, "login": account.login })
}

fn end(end: ConnectEnd) -> &'static str {
    match end {
        ConnectEnd::Expired => "expired",
        ConnectEnd::Denied => "denied",
        ConnectEnd::Cancelled => "cancelled",
        ConnectEnd::Rejected => "rejected",
        ConnectEnd::Unreachable => "unreachable",
        ConnectEnd::StoreUnavailable => "store_unavailable",
        ConnectEnd::AuditUnavailable => "audit_unavailable",
    }
}

/// Who decided the event: the caller, or GitHub's answer to the attempt.
fn cause(event: &GithubAuditEvent) -> &'static str {
    match event {
        GithubAuditEvent::Connected { .. } | GithubAuditEvent::ConnectEnded(_) => "github_answered",
        GithubAuditEvent::ConnectRequested
        | GithubAuditEvent::ConnectCancelled { .. }
        | GithubAuditEvent::DisconnectRequested { .. }
        | GithubAuditEvent::Disconnected
        | GithubAuditEvent::DisconnectFailed => "caller_requested",
    }
}

fn stored(record: &GithubAuditRecord) -> Value {
    let detail = match &record.event {
        GithubAuditEvent::Connected { account: signed_in } => {
            json!({ "account": account(signed_in) })
        }
        GithubAuditEvent::DisconnectRequested { account: held } => {
            json!({ "account": account(held) })
        }
        GithubAuditEvent::ConnectEnded(reason) => json!({ "reason": end(*reason) }),
        GithubAuditEvent::ConnectCancelled { by } => json!({ "by": caller(by) }),
        GithubAuditEvent::ConnectRequested
        | GithubAuditEvent::Disconnected
        | GithubAuditEvent::DisconnectFailed => Value::Null,
    };
    json!({
        "kind": "github_sign_in",
        "operationId": record.operation_id,
        "event": name(&record.event),
        "detail": detail,
        "cause": cause(&record.event),
        "initiator": caller(&record.caller),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;
    impl Clock for Fixed {
        fn unix_milliseconds(&self) -> u64 {
            7
        }
    }

    #[tokio::test]
    async fn each_event_is_one_private_file_naming_the_caller_and_no_secret() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("github");
        let audit = DurableGithubAudit::new(directory.clone(), Arc::new(Fixed));
        let owner = GithubCaller {
            organization_id: "org".into(),
            principal_id: "owner".into(),
            credential_id: "panel".into(),
        };
        let mut record = GithubAuditRecord {
            operation_id: "op".into(),
            caller: owner.clone(),
            event: GithubAuditEvent::ConnectRequested,
        };
        assert!(!directory.exists());
        audit.record(&record).await.unwrap();
        record.event = GithubAuditEvent::Connected {
            account: GithubAccount {
                id: 7,
                login: "octo".into(),
            },
        };
        audit.record(&record).await.unwrap();
        assert_eq!(
            audit.record(&record).await,
            Err(AuditUnavailable),
            "one record of each event per operation"
        );
        record.event = GithubAuditEvent::ConnectCancelled { by: owner };
        audit.record(&record).await.unwrap();
        let read = |name: &str| -> Value {
            serde_json::from_slice(&std::fs::read(directory.join(name)).unwrap()).unwrap()
        };
        let requested = read("github-op-connect-requested.json");
        assert_eq!(requested["initiator"]["principalId"], "owner");
        assert_eq!(requested["observedAtMs"], 7);
        assert_eq!(requested["cause"], "caller_requested");
        let connected = read("github-op-connected.json");
        assert_eq!(connected["detail"]["account"]["login"], "octo");
        assert_eq!(connected["cause"], "github_answered");
        assert_eq!(
            read("github-op-connect-cancelled.json")["detail"]["by"]["credentialId"],
            "panel"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.join("github-op-connected.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }
}
