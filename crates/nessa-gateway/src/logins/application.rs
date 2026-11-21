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

use super::chatgpt::application::{AccessRefused, ChatgptLogins, ChatgptStatus};
use super::chatgpt::domain::{AccountId, RefreshProblem, SignInEnd};
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
    /// The ChatGPT account Codex uses needs signing in again.
    SignInNeeded,
    /// The ChatGPT account's token could not be refreshed now, or its new
    /// tokens could not be kept.
    RefreshFailed,
}

/// What a record says happened. Never a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    /// An agent's login is about to be saved; `replacing` when one was.
    SaveRequested {
        agent: AgentId,
        replacing: bool,
    },
    /// The save did not complete; whatever was saved still is.
    SaveFailed {
        agent: AgentId,
    },
    /// The vault refused the save and then could not say what it holds:
    /// the new login may or may not be saved.
    SaveUnsettled {
        agent: AgentId,
    },
    /// The login is saved.
    Saved {
        agent: AgentId,
    },
    /// An agent's saved login is about to be deleted.
    RemoveRequested {
        agent: AgentId,
    },
    /// The delete did not complete; the login is still saved.
    RemoveFailed {
        agent: AgentId,
    },
    /// The vault refused the delete and then could not say what it holds:
    /// the login may or may not still be saved.
    RemoveUnsettled {
        agent: AgentId,
    },
    /// No login is saved for the agent now.
    Removed {
        agent: AgentId,
    },
    /// A paired machine's switch is about to be turned `on` or off.
    SwitchRequested {
        peer: PeerEnvironment,
        on: bool,
    },
    /// The switch did not change.
    SwitchFailed {
        peer: PeerEnvironment,
        on: bool,
    },
    /// The vault refused the switch and then could not say what it holds:
    /// the switch may or may not be as asked.
    SwitchUnsettled {
        peer: PeerEnvironment,
        on: bool,
    },
    /// The switch is as asked now.
    Switched {
        peer: PeerEnvironment,
        on: bool,
    },
    /// A harness of `agent` on `environment` is started with its login,
    /// under `lease`: the lease's own records name who started it.
    /// `chatgpt` names the ChatGPT account whose access token it was.
    Sent {
        environment: String,
        agent: AgentId,
        lease: String,
        chatgpt: Option<AccountId>,
    },
    /// A harness of `agent` on `environment` is started without one.
    NotSent {
        environment: String,
        agent: AgentId,
        lease: String,
        why: Withheld,
    },
    /// A ChatGPT sign-in, of `account` again or of a new one, is about to
    /// start.
    ChatgptSignInRequested {
        account: Option<AccountId>,
    },
    /// It ended, and how.
    ChatgptSignInEnded {
        account: Option<AccountId>,
        end: SignInEnd,
    },
    /// Codex is about to use `account`, or the API key when `None`.
    ChatgptUseRequested {
        account: Option<AccountId>,
    },
    ChatgptUseFailed {
        account: Option<AccountId>,
    },
    ChatgptUsed {
        account: Option<AccountId>,
    },
    /// `account` is about to be forgotten.
    ChatgptRemoveRequested {
        account: AccountId,
    },
    ChatgptRemoveFailed {
        account: AccountId,
    },
    /// Forgotten; `revoked` says whether OpenAI confirmed its refresh
    /// token revoked.
    ChatgptRemoved {
        account: AccountId,
        revoked: bool,
    },
    /// `account`'s tokens were refreshed and saved, for a lease.
    ChatgptRefreshed {
        account: AccountId,
    },
    ChatgptRefreshFailed {
        account: AccountId,
        why: RefreshProblem,
    },
}

/// One record. `operation_id` joins a request to its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginRecord {
    pub operation_id: String,
    /// `None` for a login handed out at a lease, which nobody asked for by
    /// hand: the event names the lease, whose own records name who started
    /// it.
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
    /// The change could not be recorded before it was made, so it was not.
    AuditUnavailable,
    /// The change was made, but its outcome could not be recorded: this
    /// service's record of it could not be kept, or the vault made it and
    /// then could not keep its own. What is saved now is the change: read
    /// the status again.
    ChangeUnrecorded,
    TooManyDevices,
    /// No ChatGPT account with that id is kept.
    UnknownAccount,
    /// This gateway keeps no ChatGPT accounts.
    ChatgptNotConfigured,
}

/// Which agents have a login saved, and which paired machines receive them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginsStatus {
    pub saved: BTreeMap<AgentId, bool>,
    pub devices_on: Vec<PeerEnvironment>,
    /// The ChatGPT accounts Codex can use, when this gateway keeps them.
    pub chatgpt: Option<ChatgptStatus>,
}

/// What one lease is handed: a login, when its agent should be started
/// again, and which ChatGPT account it is. A ChatGPT access token's
/// `renew_by_ms` is ten minutes before it expires. With no `login`, a
/// ChatGPT account is in use but could hand nothing now: `renew_by_ms` is
/// then already due, so the idle agent is parked and the next message asks
/// again (row L6 in `docs/design/chatgpt-sign-in.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandedLogin {
    pub login: Option<Login>,
    pub renew_by_ms: Option<u64>,
    pub account: Option<AccountId>,
}

impl HandedLogin {
    /// Nothing now, from a ChatGPT account in use: asked again once idle.
    fn retry() -> Self {
        Self {
            login: None,
            renew_by_ms: Some(0),
            account: None,
        }
    }
}

pub struct SavedLoginsService {
    logins: BTreeMap<AgentId, Arc<dyn SecretStore>>,
    devices: Arc<dyn SecretStore>,
    policy: Arc<dyn LoginPolicyEvaluator>,
    audit: Arc<dyn LoginAudit>,
    /// Changes one at a time, so each sees what the last one left.
    changes: tokio::sync::Mutex<()>,
    /// Told after an agent's login is saved or removed, so whatever was
    /// started with the old one can start again with the new.
    changed: Arc<dyn Fn(AgentId) + Send + Sync>,
    /// The ChatGPT accounts Codex can use instead of its saved key.
    chatgpt: Option<Arc<ChatgptLogins>>,
}

/// What a store that refused a change holds now. A store can make a change
/// and still refuse it: the vault writes the entry, then cannot keep its own
/// record of the write.
enum Settled {
    /// It holds what was asked: the change was made.
    Made,
    /// It holds something else: the change was not made.
    NotMade,
    /// It cannot say.
    Unknown,
}

fn settled(store: &dyn SecretStore, wanted: Option<&[u8]>) -> Settled {
    match store.read() {
        Ok(held) if held.as_deref().map(Vec::as_slice) == wanted => Settled::Made,
        Ok(_) => Settled::NotMade,
        Err(_) => Settled::Unknown,
    }
}

/// The three ways a refused change can end, recorded as what happened.
struct Ends {
    made: LoginEvent,
    failed: LoginEvent,
    unsettled: LoginEvent,
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
            changed: Arc::new(|_| {}),
            chatgpt: None,
        }
    }

    /// These logins, with `chatgpt`'s accounts for Codex.
    #[must_use]
    pub fn with_chatgpt(mut self, chatgpt: Arc<ChatgptLogins>) -> Self {
        self.chatgpt = Some(chatgpt);
        self
    }

    /// The ChatGPT accounts, when this gateway keeps them.
    ///
    /// # Errors
    /// [`LoginsError::ChatgptNotConfigured`].
    pub fn chatgpt(&self) -> Result<&Arc<ChatgptLogins>, LoginsError> {
        self.chatgpt
            .as_ref()
            .ok_or(LoginsError::ChatgptNotConfigured)
    }

    /// These logins, telling `changed` after each agent's login is saved or
    /// removed.
    #[must_use]
    pub fn on_change(mut self, changed: Arc<dyn Fn(AgentId) + Send + Sync>) -> Self {
        self.changed = changed;
        self
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

    /// The outcome's record, once the change was made: one that cannot be
    /// kept is [`LoginsError::ChangeUnrecorded`], never a refusal, since
    /// the change stands.
    fn record_outcome(
        &self,
        operation_id: &str,
        caller: &LoginCaller,
        event: LoginEvent,
    ) -> Result<(), LoginsError> {
        self.record(operation_id, Some(caller), event).map_err(|_| {
            tracing::error!(
                operation_id,
                "a saved-login change was made but not recorded"
            );
            LoginsError::ChangeUnrecorded
        })
    }

    /// A failure's record, once the change was refused: the failure is what
    /// the caller hears, and a record of it that cannot be kept is logged.
    fn record_failure(&self, operation_id: &str, caller: &LoginCaller, event: LoginEvent) {
        if self.record(operation_id, Some(caller), event).is_err() {
            tracing::error!(operation_id, "a saved-login failure could not be recorded");
        }
    }

    /// The answer to a change `store` refused with `problem`, recorded as
    /// what its read-back shows. A change that may have been made tells
    /// `changed` for `agent`: starting again with the same login costs a
    /// restart, keeping an old one sends a login that is gone.
    fn refused(
        &self,
        operation_id: &str,
        caller: &LoginCaller,
        problem: SecretUnavailable,
        held: Settled,
        agent: Option<AgentId>,
        ends: Ends,
    ) -> LoginsError {
        let tell = || {
            if let Some(agent) = agent {
                (self.changed)(agent);
            }
        };
        match held {
            Settled::Made => {
                tell();
                // The vault's own record of the change is lost whether or
                // not this one is kept.
                let _ = self.record_outcome(operation_id, caller, ends.made);
                LoginsError::ChangeUnrecorded
            }
            Settled::NotMade => {
                self.record_failure(operation_id, caller, ends.failed);
                store_error(problem)
            }
            Settled::Unknown => {
                tell();
                self.record_failure(operation_id, caller, ends.unsettled);
                store_error(problem)
            }
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
            chatgpt: self
                .chatgpt
                .as_ref()
                .map(|chatgpt| chatgpt.status())
                .transpose()?,
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
            let held = settled(store.as_ref(), Some(login.expose().as_bytes()));
            return Err(self.refused(
                &operation_id,
                &caller,
                problem,
                held,
                Some(agent),
                Ends {
                    made: LoginEvent::Saved { agent },
                    failed: LoginEvent::SaveFailed { agent },
                    unsettled: LoginEvent::SaveUnsettled { agent },
                },
            ));
        }
        (self.changed)(agent);
        // Made but not recorded is never answered as a success.
        self.record_outcome(&operation_id, &caller, LoginEvent::Saved { agent })?;
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
            let held = settled(store.as_ref(), None);
            return Err(self.refused(
                &operation_id,
                &caller,
                problem,
                held,
                Some(agent),
                Ends {
                    made: LoginEvent::Removed { agent },
                    failed: LoginEvent::RemoveFailed { agent },
                    unsettled: LoginEvent::RemoveUnsettled { agent },
                },
            ));
        }
        (self.changed)(agent);
        self.record_outcome(&operation_id, &caller, LoginEvent::Removed { agent })?;
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
        let wanted = encode_switches(&next);
        if let Err(problem) = self.devices.write(&wanted) {
            let held = settled(self.devices.as_ref(), Some(&wanted));
            // A switch is read at each lease: nothing started needs telling.
            return Err(self.refused(
                &operation_id,
                &caller,
                problem,
                held,
                None,
                Ends {
                    made: LoginEvent::Switched { peer, on },
                    failed: LoginEvent::SwitchFailed { peer, on },
                    unsettled: LoginEvent::SwitchUnsettled { peer, on },
                },
            ));
        }
        self.record_outcome(&operation_id, &caller, LoginEvent::Switched { peer, on })?;
        self.status()
    }

    /// The saved login a harness of `agent` started on `environment` is
    /// given: only when one is saved and the policy lets that machine
    /// receive it. Recorded either way, except when none is saved: there is
    /// nothing to hand or withhold. A login whose handing cannot be
    /// recorded is not handed, and a record that cannot be kept is logged as
    /// an error: the lease still starts, with no login, so a machine signed
    /// in on its own still works while the audit is down.
    pub fn login_for(
        &self,
        environment: &EnvironmentRef,
        agent: AgentId,
        lease: &str,
    ) -> Option<Login> {
        let operation_id = Uuid::new_v4().to_string();
        let login = match self.saved(agent) {
            Ok(Some(login)) => login,
            Ok(None) => return None,
            Err(LoginsError::VaultLocked) => {
                return self.withheld(&operation_id, environment, agent, lease, Withheld::Locked)
            }
            Err(_) => {
                return self.withheld(
                    &operation_id,
                    environment,
                    agent,
                    lease,
                    Withheld::Unavailable,
                )
            }
        };
        self.admit(&operation_id, environment, agent, lease)?;
        self.sent(&operation_id, environment, agent, lease, None)
            .then_some(login)
    }

    /// The login a harness of `agent` started on `environment` under
    /// `lease` is given: for Codex, the access token of the ChatGPT account
    /// it uses when one is chosen ([`ChatgptLogins::access`], refreshed
    /// first when it must be), else the saved key as [`Self::login_for`]
    /// hands it. A ChatGPT account that cannot give a token hands nothing:
    /// the key is never used in its place, because OpenAI says a plan
    /// error stops inference rather than switching how it is paid for.
    pub async fn hand(
        self: Arc<Self>,
        environment: EnvironmentRef,
        agent: AgentId,
        lease: String,
    ) -> Option<HandedLogin> {
        let chatgpt = match (&self.chatgpt, agent) {
            (Some(chatgpt), AgentId::Codex) => Some(chatgpt.clone()),
            _ => None,
        };
        let uses_chatgpt = match &chatgpt {
            Some(chatgpt) => {
                let check = chatgpt.clone();
                tokio::task::spawn_blocking(move || check.in_use())
                    .await
                    .unwrap_or(Err(LoginsError::StorageUnavailable))
            }
            None => Ok(false),
        };
        let (Some(chatgpt), Ok(true)) = (chatgpt, &uses_chatgpt) else {
            // The vault read and the audit write block: off the executor.
            let this = self.clone();
            return tokio::task::spawn_blocking(move || {
                let operation_id = Uuid::new_v4().to_string();
                match uses_chatgpt {
                    Err(LoginsError::VaultLocked) => {
                        this.withheld(&operation_id, &environment, agent, &lease, Withheld::Locked)
                    }
                    Err(_) => this.withheld(
                        &operation_id,
                        &environment,
                        agent,
                        &lease,
                        Withheld::Unavailable,
                    ),
                    Ok(_) => this.login_for(&environment, agent, &lease),
                }
                .map(|login| HandedLogin {
                    login: Some(login),
                    renew_by_ms: None,
                    account: None,
                })
            })
            .await
            .ok()
            .flatten();
        };
        let operation_id = Uuid::new_v4().to_string();
        // The policy first: a machine that may not receive logins never
        // costs a refresh.
        let admitted = {
            let this = self.clone();
            let (operation_id, environment, lease) =
                (operation_id.clone(), environment.clone(), lease.clone());
            tokio::task::spawn_blocking(move || {
                this.admit(&operation_id, &environment, agent, &lease)
                    .is_some()
            })
            .await
            .unwrap_or(false)
        };
        if !admitted {
            return None;
        }
        let access = chatgpt.access().await;
        let this = self.clone();
        tokio::task::spawn_blocking(move || match access {
            Ok(Some(access)) => this
                .sent(
                    &operation_id,
                    &environment,
                    agent,
                    &lease,
                    Some(access.account.clone()),
                )
                .then_some(HandedLogin {
                    login: Some(access.login),
                    renew_by_ms: Some(access.renew_by_ms),
                    account: Some(access.account),
                }),
            // Set aside a moment ago: the key, as if none were in use.
            Ok(None) => this
                .login_for(&environment, agent, &lease)
                .map(|login| HandedLogin {
                    login: Some(login),
                    renew_by_ms: None,
                    account: None,
                }),
            // Never the key in its place (row R9); asked again once idle.
            Err(refused) => {
                let _: Option<()> = this.withheld(
                    &operation_id,
                    &environment,
                    agent,
                    &lease,
                    match refused {
                        AccessRefused::Locked => Withheld::Locked,
                        AccessRefused::Unavailable => Withheld::Unavailable,
                        AccessRefused::SignInNeeded => Withheld::SignInNeeded,
                        AccessRefused::Refresh(_) => Withheld::RefreshFailed,
                    },
                );
                Some(HandedLogin::retry())
            }
        })
        .await
        .ok()
        .flatten()
    }

    /// Record that nothing is handed, and why: `None`, always.
    fn withheld<T>(
        &self,
        operation_id: &str,
        environment: &EnvironmentRef,
        agent: AgentId,
        lease: &str,
        why: Withheld,
    ) -> Option<T> {
        let name = environment.name();
        let kept = self.record(
            operation_id,
            None,
            LoginEvent::NotSent {
                environment: name.clone(),
                agent,
                lease: lease.to_owned(),
                why,
            },
        );
        if kept.is_err() {
            tracing::error!(
                environment = %name,
                agent = agent.name(),
                lease,
                "a withheld saved login could not be recorded"
            );
        }
        None
    }

    /// Whether the policy lets `environment` receive `agent`'s login; a no
    /// is recorded as withheld.
    fn admit(
        &self,
        operation_id: &str,
        environment: &EnvironmentRef,
        agent: AgentId,
        lease: &str,
    ) -> Option<()> {
        let switched_on = match environment {
            EnvironmentRef::Peer(peer) => match self.switches() {
                Ok(switches) => switches.is_on(peer),
                Err(LoginsError::VaultLocked) => {
                    return self.withheld(operation_id, environment, agent, lease, Withheld::Locked)
                }
                Err(_) => {
                    return self.withheld(
                        operation_id,
                        environment,
                        agent,
                        lease,
                        Withheld::Unavailable,
                    )
                }
            },
            _ => false,
        };
        let facts = LoginFacts {
            environment,
            agent,
            switched_on,
        };
        match self.policy.may_receive(&facts) {
            Ok(true) => Some(()),
            Ok(false) => self.withheld(
                operation_id,
                environment,
                agent,
                lease,
                Withheld::NotAllowed,
            ),
            Err(_) => self.withheld(
                operation_id,
                environment,
                agent,
                lease,
                Withheld::Unavailable,
            ),
        }
    }

    /// Record that the login is handed; `false` when the record cannot be
    /// kept, and then it is not handed.
    fn sent(
        &self,
        operation_id: &str,
        environment: &EnvironmentRef,
        agent: AgentId,
        lease: &str,
        chatgpt: Option<AccountId>,
    ) -> bool {
        let name = environment.name();
        let kept = self.record(
            operation_id,
            None,
            LoginEvent::Sent {
                environment: name.clone(),
                agent,
                lease: lease.to_owned(),
                chatgpt,
            },
        );
        if kept.is_err() {
            tracing::error!(
                environment = %name,
                agent = agent.name(),
                lease,
                "a saved login was not handed: its handoff could not be recorded"
            );
        }
        kept.is_ok()
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
