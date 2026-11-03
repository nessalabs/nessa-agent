//! `SavedLoginsService` over memory vault entries, the checked-in Cedar
//! policy and a recording audit; the switch list's stored shape; and the
//! durable audit's files.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use nessa_auth::application::ports::{AccessError, Clock};
use nessa_protocol::{agents::AgentId, lease::Login};
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, PeerEnvironment, SshDestination};
use serde_json::Value;
use zeroize::Zeroizing;

use super::application::{
    AuditUnavailable, LoginAudit, LoginCaller, LoginEvent, LoginFacts, LoginPolicyEvaluator,
    LoginRecord, LoginsError, SavedLoginsService, Withheld,
};
use super::domain::{DeviceSwitches, InvalidSwitches, MAX_SWITCHED_ON};
use super::infrastructure::{cedar::CedarLoginPolicyEvaluator, DurableLoginAudit};
use crate::secrets::{SecretStore, SecretUnavailable};

#[derive(Default)]
struct MemorySecret {
    value: Mutex<Option<Vec<u8>>>,
    locked: Mutex<bool>,
    writes_fail: Mutex<bool>,
}

impl SecretStore for MemorySecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        if *self.locked.lock().unwrap() {
            return Err(SecretUnavailable::Locked);
        }
        Ok(self.value.lock().unwrap().clone().map(Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        if *self.locked.lock().unwrap() {
            return Err(SecretUnavailable::Locked);
        }
        if *self.writes_fail.lock().unwrap() {
            return Err(SecretUnavailable::Unavailable);
        }
        *self.value.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}

#[derive(Default)]
struct Recording {
    records: Mutex<Vec<LoginRecord>>,
    refuses: Mutex<bool>,
}

impl LoginAudit for Recording {
    fn record(&self, record: &LoginRecord) -> Result<(), AuditUnavailable> {
        if *self.refuses.lock().unwrap() {
            return Err(AuditUnavailable);
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
}

impl Recording {
    fn events(&self) -> Vec<LoginEvent> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.event.clone())
            .collect()
    }
}

struct Unavailable;
impl LoginPolicyEvaluator for Unavailable {
    fn may_receive(&self, _: &LoginFacts<'_>) -> Result<bool, AccessError> {
        Err(AccessError::Unavailable)
    }
}

struct Fixture {
    logins: BTreeMap<AgentId, Arc<MemorySecret>>,
    devices: Arc<MemorySecret>,
    audit: Arc<Recording>,
    service: SavedLoginsService,
}

fn fixture_with(policy: Arc<dyn LoginPolicyEvaluator>) -> Fixture {
    let logins: BTreeMap<AgentId, Arc<MemorySecret>> = AgentId::ALL
        .iter()
        .map(|agent| (*agent, Arc::new(MemorySecret::default())))
        .collect();
    let devices = Arc::new(MemorySecret::default());
    let audit = Arc::new(Recording::default());
    let service = SavedLoginsService::new(
        logins
            .iter()
            .map(|(agent, store)| (*agent, store.clone() as Arc<dyn SecretStore>))
            .collect(),
        devices.clone(),
        policy,
        audit.clone(),
    );
    Fixture {
        logins,
        devices,
        audit,
        service,
    }
}

fn fixture() -> Fixture {
    fixture_with(Arc::new(CedarLoginPolicyEvaluator::new().unwrap()))
}

fn caller() -> LoginCaller {
    LoginCaller {
        organization_id: "org".into(),
        principal_id: "owner".into(),
        credential_id: "desktop".into(),
    }
}

fn login(text: &str) -> Login {
    Login::new(text.to_owned()).unwrap()
}

fn peer(byte: u8) -> PeerEnvironment {
    PeerEnvironment::new([byte; PeerEnvironment::KEY_BYTES])
}

fn ssh() -> EnvironmentRef {
    EnvironmentRef::Ssh(SshDestination::new("studio").unwrap())
}

fn sent(login: Option<Login>) -> Option<String> {
    login.map(|login| login.expose().to_owned())
}

/// Saving, replacing and removing a login are each recorded before and
/// after, and the status says only which agents have one.
#[tokio::test]
async fn a_login_is_saved_replaced_and_removed_each_step_recorded() {
    let fixture = fixture();
    let status = fixture
        .service
        .save(caller(), AgentId::Claude, login("first"))
        .await
        .unwrap();
    assert!(status.saved[&AgentId::Claude]);
    assert!(!status.saved[&AgentId::Codex]);
    fixture
        .service
        .save(caller(), AgentId::Claude, login("second"))
        .await
        .unwrap();
    assert_eq!(
        fixture.logins[&AgentId::Claude]
            .value
            .lock()
            .unwrap()
            .as_deref(),
        Some(b"second".as_slice())
    );
    let status = fixture
        .service
        .remove(caller(), AgentId::Claude)
        .await
        .unwrap();
    assert!(!status.saved[&AgentId::Claude]);
    // Nothing left to remove is no change, and no record.
    fixture
        .service
        .remove(caller(), AgentId::Claude)
        .await
        .unwrap();
    let agent = AgentId::Claude;
    assert_eq!(
        fixture.audit.events(),
        [
            LoginEvent::SaveRequested {
                agent,
                replacing: false
            },
            LoginEvent::Saved { agent },
            LoginEvent::SaveRequested {
                agent,
                replacing: true
            },
            LoginEvent::Saved { agent },
            LoginEvent::RemoveRequested { agent },
            LoginEvent::Removed { agent },
        ]
    );
    // A request and its outcome share one operation; caller named on both.
    let records = fixture.audit.records.lock().unwrap();
    assert_eq!(records[0].operation_id, records[1].operation_id);
    assert_ne!(records[0].operation_id, records[2].operation_id);
    assert!(records.iter().all(|record| record.caller == Some(caller())));
}

/// A change the audit cannot record is not made; a save the vault refuses
/// is recorded failed and says why.
#[tokio::test]
async fn an_unrecordable_or_refused_save_changes_nothing() {
    let fixture = fixture();
    *fixture.audit.refuses.lock().unwrap() = true;
    assert_eq!(
        fixture
            .service
            .save(caller(), AgentId::Codex, login("key"))
            .await,
        Err(LoginsError::AuditUnavailable)
    );
    assert!(fixture.logins[&AgentId::Codex]
        .value
        .lock()
        .unwrap()
        .is_none());
    *fixture.audit.refuses.lock().unwrap() = false;
    *fixture.logins[&AgentId::Codex].writes_fail.lock().unwrap() = true;
    assert_eq!(
        fixture
            .service
            .save(caller(), AgentId::Codex, login("key"))
            .await,
        Err(LoginsError::StorageUnavailable)
    );
    assert_eq!(
        fixture.audit.events().last(),
        Some(&LoginEvent::SaveFailed {
            agent: AgentId::Codex
        })
    );
    *fixture.logins[&AgentId::Codex].locked.lock().unwrap() = true;
    assert_eq!(fixture.service.status(), Err(LoginsError::VaultLocked));
}

/// Each paired machine's switch is off until turned on, is kept in the
/// vault, and a switch already as asked is no change.
#[tokio::test]
async fn a_device_switch_is_kept_in_the_vault_and_recorded() {
    let fixture = fixture();
    assert!(fixture.service.status().unwrap().devices_on.is_empty());
    let status = fixture
        .service
        .set_device(caller(), peer(1), true)
        .await
        .unwrap();
    assert_eq!(status.devices_on, [peer(1)]);
    fixture
        .service
        .set_device(caller(), peer(1), true)
        .await
        .unwrap();
    let stored = fixture.devices.value.lock().unwrap().clone().unwrap();
    assert_eq!(
        DeviceSwitches::decode(&stored)
            .unwrap()
            .on()
            .collect::<Vec<_>>(),
        [&peer(1)]
    );
    let status = fixture
        .service
        .set_device(caller(), peer(1), false)
        .await
        .unwrap();
    assert!(status.devices_on.is_empty());
    assert_eq!(
        fixture.audit.events(),
        [
            LoginEvent::SwitchRequested {
                peer: peer(1),
                on: true
            },
            LoginEvent::Switched {
                peer: peer(1),
                on: true
            },
            LoginEvent::SwitchRequested {
                peer: peer(1),
                on: false
            },
            LoginEvent::Switched {
                peer: peer(1),
                on: false
            },
        ]
    );
    *fixture.devices.locked.lock().unwrap() = true;
    assert_eq!(
        fixture.service.set_device(caller(), peer(2), true).await,
        Err(LoginsError::VaultLocked)
    );
}

/// An SSH host and a Boat workspace are handed the saved login; this
/// machine never is, and a paired one only once switched on. Every answer
/// is recorded, never with the login.
#[tokio::test]
async fn a_login_is_handed_only_where_the_policy_allows() {
    let fixture = fixture();
    fixture
        .service
        .save(caller(), AgentId::Claude, login("sk-ant-oat-secret"))
        .await
        .unwrap();
    fixture.audit.records.lock().unwrap().clear();
    let agent = AgentId::Claude;
    assert_eq!(
        sent(fixture.service.login_for(&ssh(), agent)),
        Some("sk-ant-oat-secret".to_owned())
    );
    assert_eq!(
        sent(fixture.service.login_for(&EnvironmentRef::Boat, agent)),
        Some("sk-ant-oat-secret".to_owned())
    );
    assert_eq!(
        fixture.service.login_for(&EnvironmentRef::Here, agent),
        None
    );
    let paired = EnvironmentRef::Peer(peer(3));
    assert_eq!(fixture.service.login_for(&paired, agent), None);
    fixture
        .service
        .set_device(caller(), peer(3), true)
        .await
        .unwrap();
    assert_eq!(
        sent(fixture.service.login_for(&paired, agent)),
        Some("sk-ant-oat-secret".to_owned())
    );
    // Another paired machine stays off.
    assert_eq!(
        fixture
            .service
            .login_for(&EnvironmentRef::Peer(peer(4)), agent),
        None
    );
    // An agent with none saved is handed none, and nothing is recorded.
    assert_eq!(fixture.service.login_for(&ssh(), AgentId::Codex), None);
    let handed: Vec<_> = fixture
        .audit
        .events()
        .into_iter()
        .filter(|event| matches!(event, LoginEvent::Sent { .. } | LoginEvent::NotSent { .. }))
        .collect();
    assert_eq!(
        handed,
        [
            LoginEvent::Sent {
                environment: "studio".into(),
                agent
            },
            LoginEvent::Sent {
                environment: "boat:thread".into(),
                agent
            },
            LoginEvent::NotSent {
                environment: "here".into(),
                agent,
                why: Withheld::NotAllowed
            },
            LoginEvent::NotSent {
                environment: peer(3).name(),
                agent,
                why: Withheld::NotAllowed
            },
            LoginEvent::Sent {
                environment: peer(3).name(),
                agent
            },
            LoginEvent::NotSent {
                environment: peer(4).name(),
                agent,
                why: Withheld::NotAllowed
            },
        ]
    );
    assert!(fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| matches!(
            record.event,
            LoginEvent::Sent { .. } | LoginEvent::NotSent { .. }
        ))
        .all(|record| record.caller.is_none() && !format!("{record:?}").contains("secret")));
}

/// A locked vault, a policy that cannot answer, a saved entry that is not
/// a login's shape, or a send that cannot be recorded hands nothing.
#[tokio::test]
async fn nothing_is_handed_when_any_step_cannot_answer() {
    let fixture = fixture();
    fixture
        .service
        .save(caller(), AgentId::Codex, login("sk-key"))
        .await
        .unwrap();
    *fixture.logins[&AgentId::Codex].locked.lock().unwrap() = true;
    assert_eq!(fixture.service.login_for(&ssh(), AgentId::Codex), None);
    assert!(matches!(
        fixture.audit.events().last(),
        Some(LoginEvent::NotSent {
            why: Withheld::Locked,
            ..
        })
    ));
    *fixture.logins[&AgentId::Codex].locked.lock().unwrap() = false;
    *fixture.devices.locked.lock().unwrap() = true;
    assert_eq!(
        fixture
            .service
            .login_for(&EnvironmentRef::Peer(peer(1)), AgentId::Codex),
        None
    );
    *fixture.devices.locked.lock().unwrap() = false;
    *fixture.logins[&AgentId::Codex].value.lock().unwrap() = Some(b"two words".to_vec());
    assert_eq!(fixture.service.login_for(&ssh(), AgentId::Codex), None);
    *fixture.logins[&AgentId::Codex].value.lock().unwrap() = Some(b"sk-key".to_vec());
    *fixture.audit.refuses.lock().unwrap() = true;
    assert_eq!(fixture.service.login_for(&ssh(), AgentId::Codex), None);

    let unavailable = fixture_with(Arc::new(Unavailable));
    *unavailable.logins[&AgentId::Codex].value.lock().unwrap() = Some(b"sk-key".to_vec());
    assert_eq!(unavailable.service.login_for(&ssh(), AgentId::Codex), None);
    assert_eq!(
        unavailable.audit.events(),
        [LoginEvent::NotSent {
            environment: "studio".into(),
            agent: AgentId::Codex,
            why: Withheld::Unavailable
        }]
    );
}

/// The switch list reads back what it wrote, and refuses anything else:
/// another shape, a name that is not a paired machine's, one named twice,
/// or more than the bound.
#[test]
fn the_switch_list_reads_back_only_what_it_wrote() {
    let switches = DeviceSwitches::default()
        .switched(peer(2), true)
        .unwrap()
        .switched(peer(1), true)
        .unwrap();
    let bytes = switches.encode();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        serde_json::json!({"on": [peer(1).name(), peer(2).name()]})
    );
    assert_eq!(DeviceSwitches::decode(&bytes), Ok(switches));
    let twice = serde_json::json!({"on": [peer(1).name(), peer(1).name()]}).to_string();
    for bad in [
        "{}".to_owned(),
        r#"{"on":["studio"]}"#.to_owned(),
        r#"{"on":[],"more":1}"#.to_owned(),
        twice,
    ] {
        assert_eq!(
            DeviceSwitches::decode(bad.as_bytes()),
            Err(InvalidSwitches),
            "{bad}"
        );
    }
    let mut full = DeviceSwitches::default();
    for index in 0..MAX_SWITCHED_ON {
        let mut key = [0; PeerEnvironment::KEY_BYTES];
        key[..8].copy_from_slice(&(index as u64).to_be_bytes());
        full = full.switched(PeerEnvironment::new(key), true).unwrap();
    }
    assert_eq!(full.switched(peer(0xff), true), Err(InvalidSwitches));
    // Turning one off is always possible.
    assert!(full.switched(peer(0xff), false).is_ok());
}

struct Fixed;
impl Clock for Fixed {
    fn unix_milliseconds(&self) -> u64 {
        9
    }
}

/// Each record is its own file naming its phase; a login handed at a lease
/// names the gateway, a change names its caller, and no file holds a login.
#[test]
fn the_durable_audit_writes_one_file_per_phase_and_never_a_login() {
    let directory = tempfile::tempdir().unwrap();
    let audit = DurableLoginAudit::new(directory.path().join("logins"), Arc::new(Fixed));
    let requested = LoginRecord {
        operation_id: "op".into(),
        caller: Some(caller()),
        event: LoginEvent::SaveRequested {
            agent: AgentId::Claude,
            replacing: false,
        },
    };
    audit.record(&requested).unwrap();
    audit
        .record(&LoginRecord {
            operation_id: "lease".into(),
            caller: None,
            event: LoginEvent::NotSent {
                environment: peer(1).name(),
                agent: AgentId::Opencode,
                why: Withheld::Locked,
            },
        })
        .unwrap();
    let read = |name: &str| -> Value {
        serde_json::from_slice(&std::fs::read(directory.path().join("logins").join(name)).unwrap())
            .unwrap()
    };
    let first = read("login-op-requested.json");
    assert_eq!(first["kind"], "login_save");
    assert_eq!(first["details"]["agent"], "claude");
    assert_eq!(first["initiator"]["credentialId"], "desktop");
    assert_eq!(first["observedAtMs"], 9);
    let withheld = read("login-lease-withheld.json");
    assert_eq!(withheld["cause"], "lease_granted");
    assert_eq!(withheld["initiator"]["kind"], "gateway");
    assert_eq!(withheld["details"]["reason"], "vault_locked");
    assert_eq!(audit.record(&requested), Err(AuditUnavailable));
}

/// A switch list this build cannot read switches nothing on, hands nothing
/// to a paired machine, and is written over by the next switch turned.
#[tokio::test]
async fn an_unreadable_switch_list_is_all_off_and_can_be_written_again() {
    let fixture = fixture();
    *fixture.logins[&AgentId::Claude].value.lock().unwrap() = Some(b"sk-ant".to_vec());
    *fixture.devices.value.lock().unwrap() = Some(b"not a list".to_vec());
    assert!(fixture.service.status().unwrap().devices_on.is_empty());
    assert_eq!(
        fixture
            .service
            .login_for(&EnvironmentRef::Peer(peer(1)), AgentId::Claude),
        None
    );
    let status = fixture
        .service
        .set_device(caller(), peer(1), true)
        .await
        .unwrap();
    assert_eq!(status.devices_on, [peer(1)]);
}
