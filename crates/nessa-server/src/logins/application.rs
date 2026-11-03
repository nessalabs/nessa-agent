//! The one owner of the agents' saved logins and of who may be handed
//! them. Every change is recorded before it is made, and a record that
//! cannot be kept refuses the change; what then happened is recorded after.
//! Every login handed to a machine is recorded, and so is every one
//! withheld, with why. No record holds a login.
use std::{collections::BTreeMap, sync::Arc};

use nessa_auth::application::ports::AccessError;
use nessa_protocol::{agents::AgentId, lease::Login};
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, PeerEnvironment};
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use super::domain::{DeviceSwitches, InvalidSwitches, MAX_SWITCHED_ON};
use crate::secrets::{SecretStore, SecretUnavailable};

/// Who asked, as every record names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginCaller {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// The record could not be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditUnavailable;

/// Why a login was not handed to a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Withheld {
    /// The policy does not let this machine receive logins.
    NotAllowed,
    /// The vault is locked.
    Locked,
    /// The vault, or the policy, could not answer.
    Unavailable,
}

/// What a record says happened. Never a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    /// An agent's login is about to be saved; `replacing` when one was.
    SaveRequested { agent: AgentId, replacing: bool },
    /// The save did not complete; whatever was saved still is.
    SaveFailed { agent: AgentId },
    /// The login is saved.
    Saved { agent: AgentId },
    /// An agent's saved login is about to be deleted.
    RemoveRequested { agent: AgentId },
    /// The delete did not complete; the login is still saved.
    RemoveFailed { agent: AgentId },
    /// No login is saved for the agent now.
    Removed { agent: AgentId },
    /// A paired machine's switch is about to be turned `on` or off.
    SwitchRequested { peer: PeerEnvironment, on: bool },
    /// The switch did not change.
    SwitchFailed { peer: PeerEnvironment, on: bool },
    /// The switch is as asked now.
    Switched { peer: PeerEnvironment, on: bool },
    /// A harness of `agent` on `environment` is started with its login.
    Sent { environment: String, agent: AgentId },
    /// A harness of `agent` on `environment` is started without one.
    NotSent {
        environment: String,
        agent: AgentId,
        why: Withheld,
    },
}

/// One record. `operation_id` joins a request to its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRecord {
    pub operation_id: String,
    /// `None` for a login handed out at a lease, which nobody asked for by
    /// hand: the lease's own records name who started it.
    pub caller: Option<LoginCaller>,
    pub event: LoginEvent,
}

pub trait LoginAudit: Send + Sync {
    fn record(&self, record: &LoginRecord) -> Result<(), AuditUnavailable>;
}

/// The facts the policy decides over: what kind of machine it is and, for
/// a paired one, whether its owner switched it on. Never a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFacts<'a> {
    pub environment: &'a EnvironmentRef,
    pub agent: AgentId,
    /// For a paired machine: whether its switch is on. `false` elsewhere.
    pub switched_on: bool,
}

/// May this machine receive the agent's saved login? Answered by Cedar.
pub trait LoginPolicyEvaluator: Send + Sync {
    /// # Errors
    /// [`AccessError::Unavailable`] when the policy cannot decide.
    fn may_receive(&self, facts: &LoginFacts<'_>) -> Result<bool, AccessError>;
}

/// Why a logins method was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginsError {
    VaultLocked,
    StorageUnavailable,
    AuditUnavailable,
    TooManyDevices,
}

/// Which agents have a login saved, and which paired machines receive them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginsStatus {
    pub saved: BTreeMap<AgentId, bool>,
    pub devices_on: Vec<PeerEnvironment>,
}

pub struct SavedLoginsService {
    logins: BTreeMap<AgentId, Arc<dyn SecretStore>>,
    devices: Arc<dyn SecretStore>,
    policy: Arc<dyn LoginPolicyEvaluator>,
    audit: Arc<dyn LoginAudit>,
    /// Changes one at a time, so each sees what the last one left.
    changes: tokio::sync::Mutex<()>,
}

fn store_error(problem: SecretUnavailable) -> LoginsError {
    match problem {
        SecretUnavailable::Locked => LoginsError::VaultLocked,
        SecretUnavailable::Unavailable => LoginsError::StorageUnavailable,
    }
}

impl SavedLoginsService {
    /// `logins` holds one store per agent in [`AgentId::ALL`].
    pub fn new(
        logins: BTreeMap<AgentId, Arc<dyn SecretStore>>,
        devices: Arc<dyn SecretStore>,
        policy: Arc<dyn LoginPolicyEvaluator>,
        audit: Arc<dyn LoginAudit>,
    ) -> Self {
        Self {
            logins,
            devices,
            policy,
            audit,
            changes: tokio::sync::Mutex::new(()),
        }
    }

    fn store(&self, agent: AgentId) -> Result<&Arc<dyn SecretStore>, LoginsError> {
        self.logins
            .get(&agent)
            .ok_or(LoginsError::StorageUnavailable)
    }

    /// The agent's saved login: `None` when none is, or the entry is not a
    /// login's shape (it is never sent anywhere).
    fn saved(&self, agent: AgentId) -> Result<Option<Login>, LoginsError> {
        let bytes = self.store(agent)?.read().map_err(store_error)?;
        Ok(bytes.and_then(|bytes| {
            std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| Login::new(text.to_owned()).ok())
        }))
    }

    fn switches(&self) -> Result<DeviceSwitches, LoginsError> {
        match self.devices.read().map_err(store_error)? {
            None => Ok(DeviceSwitches::default()),
            // A list this build cannot read switches nothing on, and the
            // next switch turned writes a list it can.
            Some(bytes) => Ok(decode_switches(&bytes).unwrap_or_else(|InvalidSwitches| {
                tracing::error!("the saved-login device switches are unreadable; none is on");
                DeviceSwitches::default()
            })),
        }
    }

    fn record(
        &self,
        operation_id: &str,
        caller: Option<&LoginCaller>,
        event: LoginEvent,
    ) -> Result<(), LoginsError> {
        self.audit
            .record(&LoginRecord {
                operation_id: operation_id.to_owned(),
                caller: caller.cloned(),
                event,
            })
            .map_err(|AuditUnavailable| LoginsError::AuditUnavailable)
    }

    /// A failure's record, once the change was refused: the failure is what
    /// the caller hears, and a record of it that cannot be kept is logged.
    fn record_failure(&self, operation_id: &str, caller: &LoginCaller, event: LoginEvent) {
        if self.record(operation_id, Some(caller), event).is_err() {
            tracing::error!(operation_id, "a saved-login failure could not be recorded");
        }
    }

    /// Which agents have a login saved, and which machines are switched on.
    pub fn status(&self) -> Result<LoginsStatus, LoginsError> {
        let mut saved = BTreeMap::new();
        for agent in AgentId::ALL {
            let present = self.store(*agent)?.read().map_err(store_error)?.is_some();
            saved.insert(*agent, present);
        }
        Ok(LoginsStatus {
            saved,
            devices_on: self.switches()?.on().copied().collect(),
        })
    }

    /// Save `login` for `agent`, replacing any saved one.
    pub async fn save(
        &self,
        caller: LoginCaller,
        agent: AgentId,
        login: Login,
    ) -> Result<LoginsStatus, LoginsError> {
        let _change = self.changes.lock().await;
        let store = self.store(agent)?;
        let replacing = store.read().map_err(store_error)?.is_some();
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::SaveRequested { agent, replacing },
        )?;
        if let Err(problem) = store.write(login.expose().as_bytes()) {
            self.record_failure(&operation_id, &caller, LoginEvent::SaveFailed { agent });
            return Err(store_error(problem));
        }
        // Made but not recorded is never answered as a success.
        self.record(&operation_id, Some(&caller), LoginEvent::Saved { agent })?;
        self.status()
    }

    /// Delete `agent`'s saved login. Nothing to delete is no change.
    pub async fn remove(
        &self,
        caller: LoginCaller,
        agent: AgentId,
    ) -> Result<LoginsStatus, LoginsError> {
        let _change = self.changes.lock().await;
        let store = self.store(agent)?;
        // A malformed entry is still an entry to delete.
        if store.read().map_err(store_error)?.is_none() {
            return self.status();
        }
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::RemoveRequested { agent },
        )?;
        if let Err(problem) = store.delete() {
            self.record_failure(&operation_id, &caller, LoginEvent::RemoveFailed { agent });
            return Err(store_error(problem));
        }
        self.record(&operation_id, Some(&caller), LoginEvent::Removed { agent })?;
        self.status()
    }

    /// Turn `peer`'s "Use my saved logins here" switch `on` or off. A switch
    /// already as asked is no change.
    pub async fn set_device(
        &self,
        caller: LoginCaller,
        peer: PeerEnvironment,
        on: bool,
    ) -> Result<LoginsStatus, LoginsError> {
        let _change = self.changes.lock().await;
        let switches = self.switches()?;
        if switches.is_on(&peer) == on {
            return self.status();
        }
        let next = switches
            .switched(peer, on)
            .map_err(|InvalidSwitches| LoginsError::TooManyDevices)?;
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::SwitchRequested { peer, on },
        )?;
        if let Err(problem) = self.devices.write(&encode_switches(&next)) {
            self.record_failure(
                &operation_id,
                &caller,
                LoginEvent::SwitchFailed { peer, on },
            );
            return Err(store_error(problem));
        }
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::Switched { peer, on },
        )?;
        self.status()
    }

    /// The saved login a harness of `agent` started on `environment` is
    /// given: only when one is saved and the policy lets that machine
    /// receive it. Recorded either way, except when none is saved: there is
    /// nothing to hand or withhold. A login whose handing cannot be
    /// recorded is not handed.
    pub fn login_for(&self, environment: &EnvironmentRef, agent: AgentId) -> Option<Login> {
        let operation_id = Uuid::new_v4().to_string();
        let name = environment.name();
        let withheld = |why: Withheld| {
            let _ = self.record(
                &operation_id,
                None,
                LoginEvent::NotSent {
                    environment: name.clone(),
                    agent,
                    why,
                },
            );
            None
        };
        let login = match self.saved(agent) {
            Ok(Some(login)) => login,
            Ok(None) => return None,
            Err(LoginsError::VaultLocked) => return withheld(Withheld::Locked),
            Err(_) => return withheld(Withheld::Unavailable),
        };
        let switched_on = match environment {
            EnvironmentRef::Peer(peer) => match self.switches() {
                Ok(switches) => switches.is_on(peer),
                Err(LoginsError::VaultLocked) => return withheld(Withheld::Locked),
                Err(_) => return withheld(Withheld::Unavailable),
            },
            _ => false,
        };
        let facts = LoginFacts {
            environment,
            agent,
            switched_on,
        };
        match self.policy.may_receive(&facts) {
            Ok(true) => {}
            Ok(false) => return withheld(Withheld::NotAllowed),
            Err(_) => return withheld(Withheld::Unavailable),
        }
        self.record(
            &operation_id,
            None,
            LoginEvent::Sent {
                environment: name.clone(),
                agent,
            },
        )
        .ok()
        .map(|()| login)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredSwitches {
    on: Vec<String>,
}

/// The bytes the `login.devices` vault entry holds for `switches`.
#[must_use]
pub fn encode_switches(switches: &DeviceSwitches) -> Vec<u8> {
    serde_json::to_vec(&StoredSwitches {
        on: switches.on().map(PeerEnvironment::name).collect(),
    })
    .unwrap_or_default()
}

/// Read back what [`encode_switches`] wrote.
///
/// # Errors
/// [`InvalidSwitches`] for anything else.
pub fn decode_switches(bytes: &[u8]) -> Result<DeviceSwitches, InvalidSwitches> {
    let stored: StoredSwitches = serde_json::from_slice(bytes).map_err(|_| InvalidSwitches)?;
    if stored.on.len() > MAX_SWITCHED_ON {
        return Err(InvalidSwitches);
    }
    let peers = stored
        .on
        .iter()
        .map(|name| PeerEnvironment::parse(name).map_err(|_| InvalidSwitches))
        .collect::<Result<Vec<_>, _>>()?;
    DeviceSwitches::from_peers(peers)
}
