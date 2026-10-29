//! Real private config files and audit records; only the whole-file parse port
//! uses a small fixture grammar, independent of gateway startup configuration.
use crate::{
    conversation::domain::{ChannelId, VmLifetime, VmLifetimeScope},
    mcp_servers::{
        application::StoreLock,
        infrastructure::{ConfigCheck, ConfigFiles, ConfigJson, OsConfigFiles, Published},
    },
    workspaces::{
        application::*,
        domain::Failure,
        infrastructure::{ConfigBoatRuntimeSettings, RuntimeSettingsAudit},
    },
};
use nessa_auth::{
    application::ports::Clock,
    domain::{OrganizationId, PrincipalId},
};
use nessa_sdk::infrastructure::clock::RuntimeClock;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::{fs::Permissions, io, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};
use tempfile::TempDir;
use tokio::sync::Semaphore;
use uuid::Uuid;
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        42
    }
}
fn caller() -> RuntimeCaller {
    RuntimeCaller {
        organization: OrganizationId::new("fixture-org").unwrap(),
        principal: PrincipalId::new("fixture-owner").unwrap(),
        credential: "fixture-credential".into(),
        operation: "fixture-request".into(),
    }
}
fn finite(seconds: u64) -> VmLifetime {
    VmLifetime::from_seconds(Some(seconds)).unwrap()
}
fn config(path: PathBuf) -> Arc<ConfigJson> {
    Arc::new(ConfigJson::new(
        Arc::new(OsConfigFiles::new(path)),
        ConfigCheck {
            limit: 4096,
            parses: Box::new(|bytes| serde_json::from_slice::<Value>(bytes).is_ok()),
        },
        Arc::new(RuntimeClock::new()),
    ))
}
fn fixture() -> (TempDir, ConfigBoatRuntimeSettings) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("config.json"),
        b"{\"native\":{\"enabled\":false},\"agents\":{\"selected\":\"codex\"}}\n",
    )
    .unwrap();
    std::fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(
        directory.path().join("config.json"),
        Permissions::from_mode(0o600),
    )
    .unwrap();
    let next = AtomicUsize::new(1);
    let settings = ConfigBoatRuntimeSettings {
        config: config(directory.path().join("config.json")),
        audit: Arc::new(
            RuntimeSettingsAudit::new(directory.path().join("audit"), Arc::new(Time)).unwrap(),
        ),
        next_id: Arc::new(move || Uuid::from_u128(next.fetch_add(1, Ordering::SeqCst) as u128)),
    };
    (directory, settings)
}
#[tokio::test]
async fn global_and_channel_choices_preserve_unrelated_config_and_explicit_unlimited() {
    let (directory, settings) = fixture();
    assert_eq!(
        settings
            .read(&VmLifetimeScope::Global)
            .await
            .unwrap()
            .effective(),
        VmLifetime::Unlimited
    );
    let channel = VmLifetimeScope::Channel(ChannelId::from_random([1; 10]));
    let other = VmLifetimeScope::Channel(ChannelId::from_random([2; 10]));
    settings
        .save(VmLifetimeScope::Global, Some(finite(14_400)), caller())
        .await
        .unwrap();
    let inherited = settings.read(&channel).await.unwrap();
    assert_eq!(inherited.channel_override(), None);
    assert_eq!(inherited.effective(), finite(14_400));
    settings
        .save(channel.clone(), Some(VmLifetime::Unlimited), caller())
        .await
        .unwrap();
    settings
        .save(VmLifetimeScope::Global, Some(finite(28_800)), caller())
        .await
        .unwrap();
    assert_eq!(
        settings.read(&channel).await.unwrap().effective(),
        VmLifetime::Unlimited
    );
    assert_eq!(
        settings.read(&other).await.unwrap().effective(),
        finite(28_800)
    );
    settings
        .save(channel.clone(), None, caller())
        .await
        .unwrap();
    assert_eq!(
        settings.read(&channel).await.unwrap().effective(),
        finite(28_800)
    );
    // A fresh adapter reads the same choices, without a token or vault access.
    let reopened = ConfigBoatRuntimeSettings {
        config: config(directory.path().join("config.json")),
        ..settings.clone()
    };
    assert_eq!(
        reopened.read(&other).await.unwrap().effective(),
        finite(28_800)
    );
    let document: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(document["native"], serde_json::json!({"enabled":false}));
    assert_eq!(document["agents"], serde_json::json!({"selected":"codex"}));
    let records = std::fs::read_dir(directory.path().join("audit"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(records.len(), 8);
    let record: Value = serde_json::from_slice(
        &std::fs::read(
            directory
                .path()
                .join("audit/00000000-0000-0000-0000-000000000004-outcome.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(record["target"]["scope"], "channel");
    assert_eq!(record["before"]["mode"], "never");
    assert_eq!(record["requested"]["mode"], "default");
    assert_eq!(record["after"]["mode"], "default");
    assert_eq!(record["outcome"], "durable");
    assert_eq!(record["initiator"]["principalId"], "fixture-owner");
    assert_eq!(record["initiator"]["credentialId"], "fixture-credential");
    assert_eq!(record["cause"], "caller_requested");
}
struct FailAudit {
    outcome: bool,
}
impl RuntimeAudit for FailAudit {
    fn record(&self, event: &RuntimeAuditEvent) -> Result<(), Failure> {
        if matches!(event, RuntimeAuditEvent::Outcome(..)) == self.outcome {
            Err(Failure::Storage)
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn audit_refusal_never_acknowledges_and_distinguishes_before_from_after_publication() {
    for fail_outcome in [false, true] {
        let (directory, mut settings) = fixture();
        let before = std::fs::read(directory.path().join("config.json")).unwrap();
        settings.audit = Arc::new(FailAudit {
            outcome: fail_outcome,
        });
        assert_eq!(
            settings
                .save(VmLifetimeScope::Global, Some(finite(3600)), caller())
                .await,
            Err(Failure::Storage)
        );
        if fail_outcome {
            assert_eq!(
                settings
                    .read(&VmLifetimeScope::Global)
                    .await
                    .unwrap()
                    .effective(),
                finite(3600)
            );
        } else {
            assert_eq!(
                std::fs::read(directory.path().join("config.json")).unwrap(),
                before
            );
        }
    }
}
#[tokio::test]
async fn invalid_stored_lifetimes_are_not_reset_or_repaired_by_a_later_edit() {
    let (directory, settings) = fixture();
    let bytes = b"{\"boat\":{\"autoStopSeconds\":0}}";
    std::fs::write(directory.path().join("config.json"), bytes).unwrap();
    assert_eq!(
        settings.read(&VmLifetimeScope::Global).await,
        Err(Failure::Storage)
    );
    assert_eq!(
        settings
            .save(
                VmLifetimeScope::Global,
                Some(VmLifetime::Unlimited),
                caller()
            )
            .await,
        Err(Failure::Storage)
    );
    assert_eq!(
        std::fs::read(directory.path().join("config.json")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read_dir(directory.path().join("audit"))
            .unwrap()
            .count(),
        0
    );
}

struct GatedAudit {
    inner: Arc<dyn RuntimeAudit>,
    entered: Semaphore,
    released: (Mutex<bool>, Condvar),
}
impl RuntimeAudit for GatedAudit {
    fn record(&self, event: &RuntimeAuditEvent) -> Result<(), Failure> {
        self.inner.record(event)?;
        if matches!(event, RuntimeAuditEvent::Requested(_)) {
            self.entered.add_permits(1);
            let (state, wake) = &self.released;
            let mut state = state.lock().unwrap();
            while !*state {
                state = wake.wait(state).unwrap();
            }
        }
        Ok(())
    }
}
struct ObserveLock {
    inner: OsConfigFiles,
    attempted: Semaphore,
    busy: AtomicBool,
}
impl ConfigFiles for ObserveLock {
    fn read(&self, limit: usize) -> io::Result<Option<Vec<u8>>> {
        self.inner.read(limit)
    }
    fn publish(&self, bytes: &[u8]) -> io::Result<Published> {
        self.inner.publish(bytes)
    }
    fn try_lock(&self) -> io::Result<Option<StoreLock>> {
        let result = self.inner.try_lock();
        self.busy
            .store(matches!(&result, Ok(None)), Ordering::SeqCst);
        self.attempted.add_permits(1);
        result
    }
}
#[tokio::test]
async fn caller_loss_keeps_publication_and_audit_owned_under_the_shared_config_lock() {
    let (directory, mut settings) = fixture();
    let audit = Arc::new(GatedAudit {
        inner: settings.audit.clone(),
        entered: Semaphore::new(0),
        released: (Mutex::new(false), Condvar::new()),
    });
    settings.audit = audit.clone();
    let caller_task = tokio::spawn({
        let settings = settings.clone();
        async move {
            settings
                .save(VmLifetimeScope::Global, Some(finite(14_400)), caller())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), audit.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    caller_task.abort();
    assert!(caller_task.await.unwrap_err().is_cancelled());
    let files = Arc::new(ObserveLock {
        inner: OsConfigFiles::new(directory.path().join("config.json")),
        attempted: Semaphore::new(0),
        busy: AtomicBool::new(false),
    });
    let writer_config = Arc::new(ConfigJson::new(
        files.clone(),
        ConfigCheck {
            limit: 4096,
            parses: Box::new(|bytes| serde_json::from_slice::<Value>(bytes).is_ok()),
        },
        Arc::new(RuntimeClock::new()),
    ));
    let writer = tokio::spawn(async move {
        let lock = writer_config.lock().await.unwrap();
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            let mut document = writer_config.document().unwrap();
            document
                .get_mut("agents")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("mcpServers".into(), serde_json::json!([]));
            assert_eq!(writer_config.publish(document).unwrap(), Published::Durable);
        })
        .await
        .unwrap();
    });
    let attempted = tokio::time::timeout(Duration::from_secs(5), files.attempted.acquire()).await;
    let held = files.busy.load(Ordering::SeqCst);
    // Release even on a failed observation, so a failing probe cannot strand a worker.
    *audit.released.0.lock().unwrap() = true;
    audit.released.1.notify_all();
    attempted.unwrap().unwrap().forget();
    assert!(
        held,
        "the caller's accepted save still holds config.json.lock"
    );
    tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    let stored: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(stored["boat"]["autoStopSeconds"], 14_400);
    assert_eq!(stored["agents"]["mcpServers"], serde_json::json!([]));
    assert!(directory
        .path()
        .join("audit/00000000-0000-0000-0000-000000000001-outcome.json")
        .exists());
}
struct PublishFault {
    inner: OsConfigFiles,
    kind: u8,
}
impl ConfigFiles for PublishFault {
    fn read(&self, limit: usize) -> io::Result<Option<Vec<u8>>> {
        self.inner.read(limit)
    }
    fn try_lock(&self) -> io::Result<Option<StoreLock>> {
        self.inner.try_lock()
    }
    fn publish(&self, bytes: &[u8]) -> io::Result<Published> {
        if self.kind == 0 {
            return Err(io::Error::other("injected refusal before publication"));
        }
        self.inner.publish(bytes)?;
        assert!(self.kind != 2, "injected panic after publication");
        Ok(Published::NotDurable)
    }
}
#[tokio::test]
async fn failed_non_durable_and_panicking_publication_keep_the_observed_effect_distinct() {
    for (kind, outcome) in [(0, "failed"), (1, "not_durable"), (2, "uncertain")] {
        let (directory, mut settings) = fixture();
        settings.config = Arc::new(ConfigJson::new(
            Arc::new(PublishFault {
                inner: OsConfigFiles::new(directory.path().join("config.json")),
                kind,
            }),
            ConfigCheck {
                limit: 4096,
                parses: Box::new(|bytes| serde_json::from_slice::<Value>(bytes).is_ok()),
            },
            Arc::new(RuntimeClock::new()),
        ));
        assert_eq!(
            settings
                .save(VmLifetimeScope::Global, Some(finite(14_400)), caller())
                .await,
            Err(Failure::Storage)
        );
        assert_eq!(
            settings
                .read(&VmLifetimeScope::Global)
                .await
                .unwrap()
                .effective(),
            if kind == 0 {
                VmLifetime::Unlimited
            } else {
                finite(14_400)
            }
        );
        let record: Value = serde_json::from_slice(
            &std::fs::read(
                directory
                    .path()
                    .join("audit/00000000-0000-0000-0000-000000000001-outcome.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["outcome"], outcome);
        assert_eq!(record["before"]["mode"], "default");
        assert_eq!(record["requested"]["seconds"], 14_400);
        match kind {
            0 => assert_eq!(record["after"]["mode"], "default"),
            1 => assert_eq!(record["after"]["seconds"], 14_400),
            _ => assert!(record["after"].is_null()),
        }
    }
}

#[tokio::test]
async fn resetting_the_global_choice_removes_only_its_config_key() {
    let (directory, settings) = fixture();
    let channel = VmLifetimeScope::Channel(ChannelId::from_random([1; 10]));
    settings
        .save(channel.clone(), Some(finite(14_400)), caller())
        .await
        .unwrap();
    settings
        .save(
            VmLifetimeScope::Global,
            Some(VmLifetime::Unlimited),
            caller(),
        )
        .await
        .unwrap();
    settings
        .save(VmLifetimeScope::Global, None, caller())
        .await
        .unwrap();
    let stored: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("config.json")).unwrap())
            .unwrap();
    assert!(stored["boat"].get("autoStopSeconds").is_none());
    assert_eq!(
        settings.read(&channel).await.unwrap().effective(),
        finite(14_400)
    );
    assert_eq!(
        settings
            .read(&VmLifetimeScope::Global)
            .await
            .unwrap()
            .effective(),
        VmLifetime::Unlimited
    );
}
