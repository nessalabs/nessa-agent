use super::*;
use nessa_vault::{
    application::{Clock as VaultClock, Vault},
    domain::Status,
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
struct Time;
impl VaultClock for Time {
    fn seconds(&self) -> u64 {
        0
    }
}
fn composed(root: &std::path::Path) -> Arc<crate::product::VaultRoutes> {
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            root.join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(root.join("audit"), Arc::new(Time))),
        Arc::new(Time),
    );
    Arc::new(crate::product::VaultRoutes {
        vault,
        migration: Arc::new(crate::vault::infrastructure::LegacyGithubMigration::new(
            root.to_path_buf(),
            Err(crate::secrets::SecretUnavailable::Unavailable),
            Err(crate::secrets::SecretUnavailable::Unavailable),
        )),
    })
}
async fn ask(peer: &mut TestPeer, id: &str, method: &str, params: Value) -> Value {
    peer.input
        .send(Ok(Message::Text(
            json!({"type":"req","id":id,"method":method,"params":params})
                .to_string()
                .into(),
        )))
        .unwrap();
    loop {
        let Message::Text(text) = peer.message().await else {
            panic!("response")
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value["type"] == "res" && value["id"] == id {
            return value;
        }
    }
}
#[tokio::test]
async fn vault_exists_without_agents_and_only_its_own_socket_close_seals_it() {
    let root = tempfile::tempdir().unwrap();
    let routes = composed(root.path());
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_vault(routes.clone());
    let session = authenticate(&state).await;
    let (socket, mut first) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
    assert_eq!(
        ask(&mut first, "1", "vault.status", json!({})).await["payload"]["state"],
        "unconfigured"
    );
    assert_eq!(
        ask(
            &mut first,
            "2",
            "vault.setup",
            json!({"passphrase":"long disposable test passphrase"})
        )
        .await["payload"]["state"],
        "unlocked"
    );
    let session = authenticate(&state).await;
    let (socket, mut other) = test_socket(None);
    let other_task = tokio::spawn(run_authenticated(socket, state, session));
    assert_eq!(
        ask(
            &mut other,
            "3",
            "vault.unlock",
            json!({"passphrase":"long disposable test passphrase"})
        )
        .await["error"]["code"],
        "vault_session_owned"
    );
    drop(other);
    tokio::time::timeout(Duration::from_secs(5), other_task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(routes.vault.status(), Ok(Status::Unlocked));
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(routes.vault.status(), Ok(Status::Locked));
}
#[tokio::test]
async fn ordinary_credential_without_manage_permission_cannot_initialize_storage() {
    let root = tempfile::tempdir().unwrap();
    let routes = composed(root.path());
    let (state, _) = fixture(MembershipRole::Member);
    let state = state.with_vault(routes.clone());
    let session = authenticate(&state).await;
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, state, session));
    assert_eq!(
        ask(
            &mut peer,
            "1",
            "vault.setup",
            json!({"passphrase":"long disposable test passphrase"})
        )
        .await["error"]["code"],
        "forbidden"
    );
    assert_eq!(routes.vault.status(), Ok(Status::Unconfigured));
    drop(peer);
    task.await.unwrap();
}

impl nessa_vault::application::ObservationClock for Time {
    fn unix_millis(&self) -> Result<u64, nessa_vault::domain::Failure> {
        Ok(1793030400000)
    }
}

struct LockAudit {
    file: FileAudit,
    fail: std::sync::atomic::AtomicBool,
}
impl nessa_vault::application::Audit for LockAudit {
    fn record(
        &self,
        record: &nessa_vault::application::AuditRecord,
    ) -> Result<(), nessa_vault::domain::Failure> {
        if record.action == "lock" && self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            Err(nessa_vault::domain::Failure::AuditUnavailable)
        } else {
            nessa_vault::application::Audit::record(&self.file, record)
        }
    }
}
#[derive(Default)]
struct GithubEvidence(std::sync::Mutex<Vec<crate::github::application::GithubAuditRecord>>);
#[async_trait::async_trait]
impl crate::github::application::GithubAudit for GithubEvidence {
    async fn record(
        &self,
        record: &crate::github::application::GithubAuditRecord,
    ) -> Result<(), crate::github::application::AuditUnavailable> {
        self.record_cleanup(record)
    }
    fn record_cleanup(
        &self,
        record: &crate::github::application::GithubAuditRecord,
    ) -> Result<(), crate::github::application::AuditUnavailable> {
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}
struct PendingGithub;
#[async_trait::async_trait]
impl crate::github::application::GithubApi for PendingGithub {
    async fn request_device_code(
        &self,
    ) -> Result<
        crate::github::application::DeviceAuthorization,
        crate::github::application::ApiFailure,
    > {
        Ok(crate::github::application::DeviceAuthorization {
            device_code: crate::github::application::DeviceCode("disposable".into()),
            user_code: "DISPOSABLE".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: Duration::from_secs(900),
            interval: Duration::from_secs(5),
        })
    }
    async fn poll(
        &self,
        _: &crate::github::application::DeviceCode,
    ) -> Result<crate::github::application::PollAnswer, crate::github::application::ApiFailure>
    {
        panic!("locked attempt must not poll")
    }
    async fn account(
        &self,
        _: &crate::github::domain::AccessToken,
    ) -> Result<crate::github::domain::GithubAccount, crate::github::application::ApiFailure> {
        panic!("locked attempt must not fetch account")
    }
}
#[tokio::test]
async fn a_failed_lock_audit_still_retires_github_with_attributed_evidence() {
    use crate::github::application::{EndCause, GithubAuditEvent};
    let root = tempfile::tempdir().unwrap();
    let audit = Arc::new(LockAudit {
        file: FileAudit::new(root.path().join("audit"), Arc::new(Time)),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            root.path().join("vault"),
            Arc::new(OsEntropy),
        )),
        audit.clone(),
        Arc::new(Time),
    );
    let routes = Arc::new(crate::product::VaultRoutes {
        vault: vault.clone(),
        migration: Arc::new(crate::vault::infrastructure::LegacyGithubMigration::new(
            root.path().to_owned(),
            Err(crate::secrets::SecretUnavailable::Unavailable),
            Err(crate::secrets::SecretUnavailable::Unavailable),
        )),
    });
    let evidence = Arc::new(GithubEvidence::default());
    let connections = Arc::new(
        crate::github::application::GithubConnections::load(
            Arc::new(PendingGithub),
            Arc::new(crate::github::infrastructure::GithubVaultAccess(
                vault.clone(),
            )),
            Arc::new(crate::github::infrastructure::GithubTokenVault::new(
                root.path().join("github"),
                Arc::new(crate::secrets::VaultSecret::new(
                    vault.clone(),
                    nessa_vault::domain::EntryName::new("github-sign-in").unwrap(),
                )),
            )),
            evidence.clone(),
            Arc::new(crate::timer::manual::ManualTimer::default()),
        )
        .await,
    );
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state
        .with_vault(routes)
        .with_github(Arc::new(crate::product::GithubRoutes {
            connections,
            install_url: "https://github.com/apps/test/installations/new".into(),
            live: None,
            changes: tokio::sync::Mutex::new(()),
        }));
    let session = authenticate(&state).await;
    let (socket, mut peer) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, state, session));
    assert_eq!(
        ask(
            &mut peer,
            "1",
            "vault.setup",
            json!({"passphrase":"disposable long vault passphrase"})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        ask(&mut peer, "2", "github.connect", json!({})).await["ok"],
        true
    );
    audit.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        ask(&mut peer, "3", "vault.lock", json!({})).await["error"]["code"],
        "vault_audit_unavailable"
    );
    assert_eq!(vault.status(), Ok(Status::Locked));
    let rows = evidence.0.lock().unwrap();
    let ended: Vec<_> = rows
        .iter()
        .filter(|row| {
            matches!(
                row.event,
                GithubAuditEvent::ConnectEnded {
                    cause: EndCause::VaultLocked,
                    ..
                }
            )
        })
        .collect();
    assert_eq!(
        ended.len(),
        1,
        "cleanup must happen even when key audit fails, without a status read triggering it"
    );
    assert_eq!(ended[0].caller, rows[0].caller);
    assert_eq!(ended[0].operation_id, rows[0].operation_id);
    drop(rows);
    drop(peer);
    task.await.unwrap();
}
