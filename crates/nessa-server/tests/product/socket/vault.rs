use super::*;
use nessa_vault::{
    application::{Clock as VaultClock, Vault},
    domain::Status,
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
// These independent gateway fixtures run the real memory-hard KDF. Share one
// machine's fixture capacity while preserving TestPeer's response deadlines;
// concurrency inside a fixture (owner races, cleanup and late replies) remains
// exercised by its own tasks. This is not a production-wide admission policy.
static VAULT_FIXTURE_CAPACITY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
async fn passphrase_change_requires_the_live_owner_and_seals_before_explicit_reunlock() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    let root = tempfile::tempdir().unwrap();
    let routes = composed(root.path());
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_vault(routes.clone());
    let session = authenticate(&state).await;
    let (socket, mut owner) = test_socket(None);
    let task = tokio::spawn(run_authenticated(socket, state.clone(), session));
    let old = "old synthetic passphrase for socket rotation";
    let new = "replacement synthetic passphrase for socket rotation";
    assert_eq!(
        ask(&mut owner, "1", "vault.setup", json!({"passphrase":old})).await["payload"]["state"],
        "unlocked"
    );
    let name = nessa_vault::domain::EntryName::new("synthetic-credential").unwrap();
    routes
        .vault
        .write(&name, b"generated-synthetic-value")
        .unwrap();
    let bytes = std::fs::read(root.path().join("vault/entries.enc")).unwrap();
    let session = authenticate(&state).await;
    let (socket, mut foreign) = test_socket(None);
    let foreign_task = tokio::spawn(run_authenticated(socket, state, session));
    assert_eq!(
        ask(
            &mut foreign,
            "2",
            "vault.changePassphrase",
            json!({"passphrase":new})
        )
        .await["error"]["code"],
        "vault_session_owned"
    );
    assert_eq!(routes.vault.status(), Ok(Status::Unlocked));
    assert_eq!(
        std::fs::read(root.path().join("vault/entries.enc")).unwrap(),
        bytes
    );
    assert_eq!(
        ask(
            &mut owner,
            "3",
            "vault.changePassphrase",
            json!({"passphrase":"aaaaaaaaaaaaaaa"})
        )
        .await["error"]["code"],
        "vault_passphrase_policy"
    );
    assert_eq!(routes.vault.status(), Ok(Status::Unlocked));
    assert_eq!(
        ask(
            &mut owner,
            "4",
            "vault.changePassphrase",
            json!({"passphrase":new})
        )
        .await["payload"]["state"],
        "locked"
    );
    assert_eq!(routes.vault.status(), Ok(Status::Locked));
    assert!(routes.vault.read(&name).is_err());
    assert_eq!(
        ask(
            &mut owner,
            "5",
            "vault.changePassphrase",
            json!({"passphrase":new})
        )
        .await["error"]["code"],
        "vault_locked"
    );
    assert_eq!(
        ask(&mut owner, "6", "vault.unlock", json!({"passphrase":old})).await["error"]["code"],
        "vault_wrong_passphrase"
    );
    assert_eq!(
        ask(&mut owner, "7", "vault.unlock", json!({"passphrase":new})).await["payload"]["state"],
        "unlocked"
    );
    assert_eq!(
        routes.vault.read(&name).unwrap().unwrap().as_slice(),
        b"generated-synthetic-value"
    );
    drop(owner);
    drop(foreign);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), foreign_task)
        .await
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn vault_exists_without_agents_and_only_its_own_socket_close_seals_it() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
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
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
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
struct GithubEvidence(
    std::sync::Mutex<Vec<crate::github::application::GithubAuditRecord>>,
    std::sync::atomic::AtomicBool,
);
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
        assert!(
            !self.1.load(std::sync::atomic::Ordering::SeqCst),
            "synthetic cleanup audit panic"
        );
        Ok(())
    }
}
struct PendingGithub;
#[async_trait::async_trait]
impl crate::github::application::GithubApi for PendingGithub {
    async fn read_repository(
        &self,
        _: &crate::github::domain::AccessToken,
        _: &crate::github::domain::GithubRepository,
        _: crate::github::domain::GithubReadKind,
    ) -> Result<serde_json::Value, crate::github::application::GithubReadFailure> {
        panic!("locked attempt must not read repository")
    }

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
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    assert_failed_seal_retires_github(
        "vault.lock",
        json!({}),
        "vault_audit_unavailable",
        SealFailure::KeyAudit,
    )
    .await;
}
#[tokio::test]
async fn a_failed_passphrase_change_sealing_audit_still_retires_github() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    assert_failed_seal_retires_github(
        "vault.changePassphrase",
        json!({"passphrase":"replacement synthetic vault passphrase"}),
        "vault_change_unchanged_audit_unavailable",
        SealFailure::KeyAudit,
    )
    .await;
}
#[tokio::test]
async fn a_panicked_passphrase_change_still_retires_github() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    assert_failed_seal_retires_github(
        "vault.changePassphrase",
        json!({"passphrase":"replacement synthetic vault passphrase"}),
        "vault_change_not_confirmed",
        SealFailure::RepositoryPanic,
    )
    .await;
}
#[derive(Clone, Copy)]
enum SealFailure {
    KeyAudit,
    RepositoryPanic,
    CleanupPanic(nessa_vault::domain::ChangePublication),
}
#[tokio::test]
async fn a_cleanup_panic_preserves_the_known_passphrase_publication() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    use nessa_vault::domain::{ChangePublication, Failure};
    for (publication, code) in [
        (
            ChangePublication::Committed,
            "vault_changed_audit_unavailable",
        ),
        (
            ChangePublication::Unchanged(Failure::Unavailable),
            "vault_change_unchanged_audit_unavailable",
        ),
    ] {
        assert_failed_seal_retires_github(
            "vault.changePassphrase",
            json!({"passphrase":"replacement synthetic vault passphrase"}),
            code,
            SealFailure::CleanupPanic(publication),
        )
        .await;
    }
}
async fn assert_failed_seal_retires_github(
    method: &str,
    params: Value,
    refusal: &str,
    fault: SealFailure,
) {
    let panic_repository = matches!(fault, SealFailure::RepositoryPanic);
    use crate::github::application::{EndCause, GithubAuditEvent};
    let root = tempfile::tempdir().unwrap();
    let audit = Arc::new(LockAudit {
        file: FileAudit::new(root.path().join("audit"), Arc::new(Time)),
        fail: std::sync::atomic::AtomicBool::new(false),
    });
    let repository: Arc<dyn nessa_vault::application::Repository> =
        if !matches!(fault, SealFailure::KeyAudit) {
            Arc::new(ChangeRepository {
                inner: EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy)),
                publication: match fault {
                    SealFailure::CleanupPanic(value) => value,
                    _ => nessa_vault::domain::ChangePublication::Committed,
                },
                panic: panic_repository,
            })
        } else {
            Arc::new(EncryptedFileVault::new(
                root.path().join("vault"),
                Arc::new(OsEntropy),
            ))
        };
    let vault = Vault::new(repository, audit.clone(), Arc::new(Time));
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
    audit.fail.store(
        matches!(fault, SealFailure::KeyAudit),
        std::sync::atomic::Ordering::SeqCst,
    );
    evidence.1.store(
        matches!(fault, SealFailure::CleanupPanic(_)),
        std::sync::atomic::Ordering::SeqCst,
    );
    assert_eq!(
        ask(&mut peer, "3", method, params).await["error"]["code"],
        refusal
    );
    if panic_repository {
        assert!(!vault.is_unlocked());
    } else {
        assert_eq!(vault.status(), Ok(Status::Locked));
    }
    {
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
    }
    drop(peer);
    task.await.unwrap();
}

#[tokio::test]
async fn a_queued_disconnect_cannot_delete_a_sign_in_restored_by_a_new_unlock() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    use crate::github::application::GithubTokenStore;
    use crate::github::domain::{AccessToken, GithubAccount, GithubSignIn, GithubStatus};
    use nessa_vault::application::Caller;
    use nessa_vault::domain::{EntryName, Passphrase};

    let root = tempfile::tempdir().unwrap();
    let vault_routes = composed(root.path());
    let owner = vault_routes.vault.session(
        "github-generation-race".into(),
        Caller {
            principal: "owner".into(),
            credential: "credential".into(),
        },
    );
    let lease = owner.lease();
    let passphrase = "disposable long vault passphrase";
    assert_eq!(
        vault_routes
            .vault
            .create(&lease, Passphrase::new(passphrase.into()).unwrap(), &[]),
        Ok(Status::Unlocked)
    );
    let old_generation = vault_routes.vault.unlocked_generation().unwrap();
    let store = Arc::new(crate::github::infrastructure::GithubTokenVault::new(
        root.path().join("github"),
        Arc::new(crate::secrets::VaultSecret::new(
            vault_routes.vault.clone(),
            EntryName::new("github-sign-in").unwrap(),
        )),
    ));
    let sign_in = GithubSignIn {
        account: GithubAccount {
            id: 7,
            login: "octo".into(),
        },
        token: AccessToken::new("fixture-token".into()).unwrap(),
    };
    store.save(old_generation, &sign_in).await.unwrap();
    let connections = Arc::new(
        crate::github::application::GithubConnections::load(
            Arc::new(PendingGithub),
            Arc::new(crate::github::infrastructure::GithubVaultAccess(
                vault_routes.vault.clone(),
            )),
            store.clone(),
            Arc::new(GithubEvidence::default()),
            Arc::new(crate::timer::manual::ManualTimer::default()),
        )
        .await,
    );
    let github = Arc::new(crate::product::GithubRoutes {
        connections: connections.clone(),
        install_url: "https://github.com/apps/test/installations/new".into(),
        live: None,
        changes: tokio::sync::Mutex::new(()),
    });
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state
        .with_vault(vault_routes.clone())
        .with_github(github.clone());
    let session = authenticate(&state).await;

    let held = github.changes.lock().await;
    let frame = RequestFrame::new("old-disconnect", "github.disconnect", &json!({})).unwrap();
    let mut queued = Box::pin(crate::product::github::dispatch(&state, &session, frame));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());

    assert_eq!(vault_routes.vault.lock(&lease), Ok(Status::Locked));
    assert_eq!(
        vault_routes
            .vault
            .unlock(&lease, Passphrase::new(passphrase.into()).unwrap()),
        Ok(Status::Unlocked)
    );
    let new_generation = vault_routes.vault.unlocked_generation().unwrap();
    assert_ne!(old_generation, new_generation);
    connections.restore().await.unwrap();
    assert!(matches!(
        connections.status(),
        GithubStatus::Connected { .. }
    ));
    drop(held);

    let reply = queued.await;
    let nessa_protocol::protocol::OutgoingMessage::Response(response) = reply else {
        panic!("response")
    };
    assert_eq!(
        response.error.as_ref().map(|error| error.code.as_str()),
        Some("vault_locked")
    );
    assert!(matches!(
        connections.status(),
        GithubStatus::Connected { .. }
    ));
    assert_eq!(store.load(new_generation).await.unwrap(), Some(sign_in));
}

#[tokio::test]
async fn an_in_flight_old_unlock_cannot_disable_live_events_or_disconnect_new_selection() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    use crate::github_relay::{
        application::{
            ConnectedGithub, GitHubCredentials, InboxKeySource, LiveSelectionAdmission, RelayStore,
        },
        domain::{Binding, Failure as RelayFailure},
        infrastructure::{SqliteRelayStore, VaultSelectionAdmission},
    };
    use nessa_vault::{application::Caller, domain::Passphrase};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Condvar, Mutex,
    };

    struct Key;
    impl InboxKeySource for Key {
        fn load(&self) -> Result<Arc<nessa_local_database::DatabaseKey>, RelayFailure> {
            Ok(Arc::new(nessa_local_database::DatabaseKey::new([11; 32])))
        }
    }
    struct Connected;
    #[async_trait::async_trait]
    impl GitHubCredentials for Connected {
        async fn token(&self) -> Result<crate::github::domain::AccessToken, RelayFailure> {
            Err(RelayFailure::CredentialsUnavailable)
        }
        async fn identify(&self, _: &str) -> Result<Binding, RelayFailure> {
            Err(RelayFailure::CredentialsUnavailable)
        }
    }
    #[async_trait::async_trait]
    impl ConnectedGithub for Connected {
        fn user_id(&self) -> Option<String> {
            Some("11".into())
        }
        async fn repositories(&self) -> Result<Vec<Binding>, RelayFailure> {
            Err(RelayFailure::CredentialsUnavailable)
        }
    }
    struct PausedAdmission {
        inner: VaultSelectionAdmission,
        pause: AtomicBool,
        entered: tokio::sync::Notify,
        released: (Mutex<bool>, Condvar),
    }
    impl PausedAdmission {
        fn arm(&self) {
            *self.released.0.lock().unwrap() = false;
            self.pause.store(true, Ordering::SeqCst);
        }
        fn release(&self) {
            *self.released.0.lock().unwrap() = true;
            self.released.1.notify_one();
        }
    }
    impl LiveSelectionAdmission for PausedAdmission {
        fn commit_in_generation(
            &self,
            generation: u64,
            commit: &mut dyn FnMut() -> Result<(), RelayFailure>,
        ) -> Result<(), RelayFailure> {
            if self.pause.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                let mut ready = self.released.0.lock().unwrap();
                while !*ready {
                    ready = self.released.1.wait(ready).unwrap();
                }
            }
            self.inner.commit_in_generation(generation, commit)
        }
    }

    let root = tempfile::tempdir().unwrap();
    let vault_routes = composed(root.path());
    let owner = vault_routes.vault.session(
        "live-selection-race".into(),
        Caller {
            principal: "owner".into(),
            credential: "credential".into(),
        },
    );
    let lease = owner.lease();
    let passphrase = "disposable long vault passphrase";
    assert_eq!(
        vault_routes
            .vault
            .create(&lease, Passphrase::new(passphrase.into()).unwrap(), &[]),
        Ok(Status::Unlocked)
    );
    let admission = Arc::new(PausedAdmission {
        inner: VaultSelectionAdmission(vault_routes.vault.clone()),
        pause: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        released: (Mutex::new(false), Condvar::new()),
    });
    let store = Arc::new(SqliteRelayStore::new(
        root.path().join("private/inbox.sqlite"),
        Arc::new(|| 1_000_000),
        Arc::new(Key),
        admission.clone(),
    ));
    let selected = Binding::new(
        "11".into(),
        "42".into(),
        "nessalabs/nessa-agent".into(),
        uuid::Uuid::new_v4(),
    )
    .unwrap();
    store.bind(&selected, false).await.unwrap();
    let live = Arc::new(crate::github_relay::application::LiveEvents::new(
        Arc::new(Connected),
        store.clone(),
    ));
    let connections = Arc::new(
        crate::github::application::GithubConnections::load(
            Arc::new(PendingGithub),
            Arc::new(crate::github::infrastructure::GithubVaultAccess(
                vault_routes.vault.clone(),
            )),
            Arc::new(crate::github::infrastructure::GithubTokenVault::new(
                root.path().join("github"),
                Arc::new(crate::secrets::VaultSecret::new(
                    vault_routes.vault.clone(),
                    nessa_vault::domain::EntryName::new("github-sign-in").unwrap(),
                )),
            )),
            Arc::new(GithubEvidence::default()),
            Arc::new(crate::timer::manual::ManualTimer::default()),
        )
        .await,
    );
    let github = Arc::new(crate::product::GithubRoutes {
        connections: connections.clone(),
        install_url: "https://github.com/apps/test/installations/new".into(),
        live: Some(live),
        changes: tokio::sync::Mutex::new(()),
    });
    let (state, _) = fixture(MembershipRole::Admin);
    let state = state.with_vault(vault_routes.clone()).with_github(github);
    let session = authenticate(&state).await;
    for (method, params) in [
        ("github.liveEvents", json!({"enabled":false})),
        ("github.disconnect", json!({})),
    ] {
        admission.arm();
        let entered = admission.entered.notified();
        let frame = RequestFrame::new(method, method, &params).unwrap();
        let mut pending = Box::pin(crate::product::github::dispatch(&state, &session, frame));
        assert!(futures_util::poll!(pending.as_mut()).is_pending());
        tokio::time::timeout(Duration::from_secs(5), entered)
            .await
            .unwrap();
        assert_eq!(vault_routes.vault.lock(&lease), Ok(Status::Locked));
        assert_eq!(
            vault_routes
                .vault
                .unlock(&lease, Passphrase::new(passphrase.into()).unwrap()),
            Ok(Status::Unlocked)
        );
        connections.restore().await.unwrap();
        admission.release();
        let nessa_protocol::protocol::OutgoingMessage::Response(response) = pending.await else {
            panic!("response")
        };
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("vault_locked")
        );
        assert_eq!(store.bindings().await.unwrap(), vec![selected.clone()]);
    }
}

struct ChangeRepository {
    inner: EncryptedFileVault,
    publication: nessa_vault::domain::ChangePublication,
    panic: bool,
}
impl nessa_vault::application::Repository for ChangeRepository {
    fn configured(&self) -> Result<bool, nessa_vault::domain::Failure> {
        self.inner.configured()
    }
    fn create(
        &self,
        p: &nessa_vault::domain::Passphrase,
        entries: &[(nessa_vault::domain::EntryName, nessa_vault::domain::Secret)],
    ) -> Result<nessa_vault::domain::VaultKey, nessa_vault::domain::Failure> {
        self.inner.create(p, entries)
    }
    fn unlock(
        &self,
        p: &nessa_vault::domain::Passphrase,
    ) -> Result<nessa_vault::domain::VaultKey, nessa_vault::domain::Failure> {
        self.inner.unlock(p)
    }
    fn read(
        &self,
        k: &nessa_vault::domain::VaultKey,
        n: &nessa_vault::domain::EntryName,
    ) -> Result<Option<nessa_vault::domain::Secret>, nessa_vault::domain::Failure> {
        self.inner.read(k, n)
    }
    fn write(
        &self,
        k: &nessa_vault::domain::VaultKey,
        n: &nessa_vault::domain::EntryName,
        v: &[u8],
    ) -> Result<(), nessa_vault::domain::Failure> {
        self.inner.write(k, n, v)
    }
    fn delete(
        &self,
        k: &nessa_vault::domain::VaultKey,
        n: &nessa_vault::domain::EntryName,
    ) -> Result<(), nessa_vault::domain::Failure> {
        self.inner.delete(k, n)
    }
    fn change_passphrase(
        &self,
        _: &nessa_vault::domain::VaultKey,
        _: &nessa_vault::domain::Passphrase,
    ) -> nessa_vault::domain::ChangePublication {
        assert!(!self.panic, "synthetic repository panic after sealing");
        self.publication
    }
}
struct ChangeAudit {
    inner: FileAudit,
    terminal_fail: bool,
}
impl nessa_vault::application::Audit for ChangeAudit {
    fn record(
        &self,
        row: &nessa_vault::application::AuditRecord,
    ) -> Result<(), nessa_vault::domain::Failure> {
        if self.terminal_fail && row.action == "passphrase_change" && row.result != "requested" {
            return Err(nessa_vault::domain::Failure::AuditUnavailable);
        }
        self.inner.record(row)
    }
}
#[tokio::test]
async fn change_wire_preserves_each_publication_fact_independently_of_audit_failure() {
    let _fixture_capacity = VAULT_FIXTURE_CAPACITY.lock().await;
    use nessa_vault::domain::{ChangePublication, Failure, Passphrase};
    for (publication, audit_failed, expected) in [
        (ChangePublication::Committed, false, None),
        (
            ChangePublication::Committed,
            true,
            Some("vault_changed_audit_unavailable"),
        ),
        (
            ChangePublication::Unchanged(Failure::Unavailable),
            false,
            Some("vault_change_unchanged"),
        ),
        (
            ChangePublication::Unchanged(Failure::Unavailable),
            true,
            Some("vault_change_unchanged_audit_unavailable"),
        ),
        (
            ChangePublication::Uncertain(Failure::Unavailable),
            false,
            Some("vault_change_not_confirmed"),
        ),
        (
            ChangePublication::Uncertain(Failure::Unavailable),
            true,
            Some("vault_change_not_confirmed_audit_unavailable"),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::new(
            Arc::new(ChangeRepository {
                inner: EncryptedFileVault::new(root.path().join("vault"), Arc::new(OsEntropy)),
                publication,
                panic: false,
            }),
            Arc::new(ChangeAudit {
                inner: FileAudit::new(root.path().join("audit"), Arc::new(Time)),
                terminal_fail: audit_failed,
            }),
            Arc::new(Time),
        );
        let owner = vault.session(
            "synthetic-owner".into(),
            nessa_vault::application::Caller {
                principal: "owner".into(),
                credential: "surface".into(),
            },
        );
        let lease = owner.lease();
        vault
            .create(
                &lease,
                Passphrase::new("synthetic original vault phrase".into()).unwrap(),
                &[],
            )
            .unwrap();
        let routes = Arc::new(crate::product::VaultRoutes {
            vault: vault.clone(),
            migration: composed(root.path()).migration.clone(),
        });
        let (state, _) = fixture(MembershipRole::Admin);
        let mut state = state.with_vault(routes);
        state.vault_lease = Some(lease);
        let frame = RequestFrame::new(
            "change",
            "vault.changePassphrase",
            &json!({"passphrase":"synthetic replacement vault phrase"}),
        )
        .unwrap();
        let response = crate::product::vault::dispatch(&state, frame).await;
        let value = serde_json::to_value(response).unwrap();
        match expected {
            Some(code) => assert_eq!(
                value["error"]["code"], code,
                "{publication:?}, audit={audit_failed}"
            ),
            None => assert_eq!(value["payload"]["state"], "locked"),
        }
        assert_eq!(vault.status(), Ok(Status::Locked));
    }
}
