//! The one owner of the ChatGPT accounts Codex uses on other machines: the
//! sign-in, the accounts kept, which one is in use, and the refreshes. Only
//! this refreshes, one at a time, so OpenAI's rotating refresh token is never
//! raced. Every change is recorded before it is made and every refresh after;
//! no record holds a token. The orderings are the table in
//! `docs/design/chatgpt-sign-in.md` (rows S*, R*, A*).
use std::sync::{Arc, Mutex};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nessa_auth::application::ports::Clock;
use nessa_protocol::lease::Login;
use sha2::{Digest, Sha256};
use tokio::task::AbortHandle;
use uuid::Uuid;

use super::domain::{
    AccountId, ChatgptAccount, ChatgptAccounts, RefreshProblem, Secret, SignInEnd, Tokens,
    UnknownAccount, PLAN_SCOPE, REGISTRATION_CLIENT, SIGN_IN_WAIT,
};
use super::ports::{
    Authorize, Callback, CallbackListener, CallbackReply, CallbackServer, ChatgptAuthority,
    RefreshFailure, TokenAnswer,
};
use crate::logins::application::{LoginAudit, LoginCaller, LoginEvent, LoginRecord, LoginsError};
use crate::secrets::{SecretStore, SecretUnavailable};

/// One account as Settings shows it. Never a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountStatus {
    pub id: AccountId,
    pub email: String,
    pub in_use: bool,
    /// OpenAI refused its refresh token: it needs signing in again.
    pub needs_sign_in: bool,
}

/// Where the sign-in stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInStatus {
    /// None under way; how the last one ended, if one did this run.
    Idle { last: Option<SignInEnd> },
    /// Waiting for the browser to come back from `authorize_url`.
    Waiting {
        authorize_url: String,
        /// The account signing in again, or `None` for a new one.
        account: Option<AccountId>,
    },
}

/// The ChatGPT accounts, and the sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatgptStatus {
    pub accounts: Vec<AccountStatus>,
    pub sign_in: SignInStatus,
}

/// A ChatGPT access token for one lease, and when its agent should be
/// started again with a fresh one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatgptAccess {
    pub account: AccountId,
    pub login: Login,
    pub renew_by_ms: u64,
}

/// Why the account in use handed no token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessRefused {
    /// The vault is locked.
    Locked,
    /// The vault could not be read.
    Unavailable,
    /// The account needs signing in again.
    SignInNeeded,
    /// OpenAI could not refresh it now, or its new tokens could not be kept.
    Refresh(RefreshProblem),
}

enum SignIn {
    Idle {
        last: Option<SignInEnd>,
    },
    Waiting {
        operation_id: String,
        caller: LoginCaller,
        account: Option<AccountId>,
        authorize_url: String,
        task: AbortHandle,
    },
}

pub struct ChatgptLogins {
    store: Arc<dyn SecretStore>,
    authority: Arc<dyn ChatgptAuthority>,
    listener: Arc<dyn CallbackListener>,
    audit: Arc<dyn LoginAudit>,
    clock: Arc<dyn Clock>,
    /// Every read-change-write of the entry, a refresh's included, one at a
    /// time: a refresh holds it across its call to OpenAI, so a second lease
    /// waits and reads the refreshed tokens (row R4).
    changes: tokio::sync::Mutex<()>,
    sign_in: Mutex<SignIn>,
}

fn store_error(problem: SecretUnavailable) -> LoginsError {
    match problem {
        SecretUnavailable::Locked => LoginsError::VaultLocked,
        SecretUnavailable::Unavailable => LoginsError::StorageUnavailable,
    }
}

/// 32 random bytes as base64url: a state, a nonce, or a PKCE verifier.
fn random_value() -> String {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random source");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What one attempt was started with, for its callback.
struct Attempt {
    operation_id: String,
    account: Option<ChatgptAccount>,
    state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
}

impl ChatgptLogins {
    pub fn new(
        store: Arc<dyn SecretStore>,
        authority: Arc<dyn ChatgptAuthority>,
        listener: Arc<dyn CallbackListener>,
        audit: Arc<dyn LoginAudit>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            authority,
            listener,
            audit,
            clock,
            changes: tokio::sync::Mutex::new(()),
            sign_in: Mutex::new(SignIn::Idle { last: None }),
        }
    }

    /// The accounts kept, `None` when none ever was.
    fn read(&self) -> Result<Option<ChatgptAccounts>, LoginsError> {
        match self.store.read().map_err(store_error)? {
            None => Ok(None),
            Some(bytes) => ChatgptAccounts::decode(&bytes).map(Some).map_err(|_| {
                tracing::error!("the saved ChatGPT accounts are unreadable");
                LoginsError::StorageUnavailable
            }),
        }
    }

    fn write(&self, accounts: &ChatgptAccounts) -> Result<(), LoginsError> {
        self.store.write(&accounts.encode()).map_err(store_error)
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
            .map_err(|_| LoginsError::AuditUnavailable)
    }

    /// An outcome's record: the change stands whether or not it is kept.
    fn record_outcome(
        &self,
        operation_id: &str,
        caller: Option<&LoginCaller>,
        event: LoginEvent,
    ) -> Result<(), LoginsError> {
        self.record(operation_id, caller, event).map_err(|_| {
            tracing::error!(
                operation_id,
                "a ChatGPT login change was made but not recorded"
            );
            LoginsError::ChangeUnrecorded
        })
    }

    /// The accounts and the sign-in, as Settings shows them.
    pub fn status(&self) -> Result<ChatgptStatus, LoginsError> {
        let accounts = self.read()?.unwrap_or_default();
        let in_use = accounts.in_use().map(|account| account.id.clone());
        let sign_in = match &*self.sign_in.lock().unwrap() {
            SignIn::Idle { last } => SignInStatus::Idle { last: *last },
            SignIn::Waiting {
                authorize_url,
                account,
                ..
            } => SignInStatus::Waiting {
                authorize_url: authorize_url.clone(),
                account: account.clone(),
            },
        };
        Ok(ChatgptStatus {
            accounts: accounts
                .accounts()
                .iter()
                .map(|account| AccountStatus {
                    id: account.id.clone(),
                    email: account.email.clone(),
                    in_use: in_use.as_ref() == Some(&account.id),
                    needs_sign_in: account.tokens.is_none(),
                })
                .collect(),
            sign_in,
        })
    }

    /// Whether Codex uses a ChatGPT account, so a lease is handed its
    /// token rather than the saved API key.
    pub fn in_use(&self) -> Result<bool, LoginsError> {
        Ok(self
            .read()?
            .is_some_and(|accounts| accounts.in_use().is_some()))
    }

    /// Start a sign-in: of `account` again, or of a new account. One under
    /// way is replaced. Answers once the browser can be sent; the sign-in
    /// finishes when it comes back (rows S1, S2, S12).
    ///
    /// # Errors
    /// [`LoginsError::UnknownAccount`] for an account not kept; the vault's
    /// and the audit's own failures, before anything started.
    pub async fn begin(
        self: &Arc<Self>,
        caller: LoginCaller,
        account: Option<AccountId>,
    ) -> Result<(), LoginsError> {
        // The host ID is kept before the first sign-in sends it.
        let (accounts, account) = {
            let _change = self.changes.lock().await;
            let accounts = match self.read()? {
                Some(accounts) => accounts,
                None => {
                    let accounts = ChatgptAccounts::new();
                    self.write(&accounts)?;
                    accounts
                }
            };
            let account = match account {
                Some(id) => Some(
                    accounts
                        .account(&id)
                        .cloned()
                        .ok_or(LoginsError::UnknownAccount)?,
                ),
                None => None,
            };
            (accounts, account)
        };
        let operation_id = Uuid::new_v4().to_string();
        let account_id = account.as_ref().map(|account| account.id.clone());
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::ChatgptSignInRequested {
                account: account_id.clone(),
            },
        )?;
        self.end_waiting(SignInEnd::Replaced, None);
        let server = match self.listener.listen().await {
            Ok(server) => server,
            Err(_) => {
                self.ended(&operation_id, &caller, account_id, SignInEnd::ListenFailed);
                return Ok(());
            }
        };
        let attempt = Attempt {
            operation_id: operation_id.clone(),
            account,
            state: random_value(),
            nonce: random_value(),
            verifier: random_value(),
            redirect_uri: server.redirect_uri(),
        };
        let challenge = challenge(&attempt.verifier);
        let authorize_url = self.authority.authorize_url(&Authorize {
            client_id: attempt
                .account
                .as_ref()
                .map_or(REGISTRATION_CLIENT, |account| account.client_id.as_str()),
            register: attempt.account.is_none(),
            host_id: accounts.host_id(),
            redirect_uri: &attempt.redirect_uri,
            state: &attempt.state,
            nonce: &attempt.nonce,
            challenge: &challenge,
            id_token_hint: attempt.account.as_ref().map(|account| &account.id_token),
            login_hint: attempt
                .account
                .as_ref()
                .map(|account| account.email.as_str()),
        });
        let mut sign_in = self.sign_in.lock().unwrap();
        let this = self.clone();
        let task = tokio::spawn(async move { this.wait(server, attempt).await }).abort_handle();
        *sign_in = SignIn::Waiting {
            operation_id,
            caller,
            account: account_id,
            authorize_url,
            task,
        };
        Ok(())
    }

    /// Stop the sign-in under way, if one is (row S14).
    pub fn cancel(&self) {
        self.end_waiting(SignInEnd::Cancelled, None);
    }

    /// End the attempt waiting now with `end`, recorded; `only` limits it to
    /// that operation.
    fn end_waiting(&self, end: SignInEnd, only: Option<&str>) {
        let ended = {
            let mut sign_in = self.sign_in.lock().unwrap();
            match &*sign_in {
                SignIn::Waiting { operation_id, .. }
                    if only.is_none_or(|only| only == operation_id) =>
                {
                    let SignIn::Waiting {
                        operation_id,
                        caller,
                        account,
                        task,
                        ..
                    } = std::mem::replace(&mut *sign_in, SignIn::Idle { last: Some(end) })
                    else {
                        unreachable!("matched as waiting above");
                    };
                    if only.is_none() {
                        task.abort();
                    }
                    Some((operation_id, caller, account))
                }
                _ => None,
            }
        };
        if let Some((operation_id, caller, account)) = ended {
            self.record_end(&operation_id, &caller, account, end);
        }
    }

    /// An attempt that never started waiting ended with `end`.
    fn ended(
        &self,
        operation_id: &str,
        caller: &LoginCaller,
        account: Option<AccountId>,
        end: SignInEnd,
    ) {
        *self.sign_in.lock().unwrap() = SignIn::Idle { last: Some(end) };
        self.record_end(operation_id, caller, account, end);
    }

    fn record_end(
        &self,
        operation_id: &str,
        caller: &LoginCaller,
        account: Option<AccountId>,
        end: SignInEnd,
    ) {
        if self
            .record(
                operation_id,
                Some(caller),
                LoginEvent::ChatgptSignInEnded { account, end },
            )
            .is_err()
        {
            tracing::error!(
                operation_id,
                end = end.name(),
                "a ChatGPT sign-in's end could not be recorded"
            );
        }
    }

    /// Wait for the browser to come back to `server` for `attempt`, then
    /// finish it (rows S3 to S13).
    async fn wait(self: Arc<Self>, mut server: Box<dyn CallbackServer>, attempt: Attempt) {
        let deadline = crate::timer::clock::sleep(SIGN_IN_WAIT);
        tokio::pin!(deadline);
        let end = loop {
            let callback = tokio::select! {
                () = &mut deadline => break SignInEnd::TimedOut,
                callback = server.next() => callback,
            };
            let Some(Callback { params, reply }) = callback else {
                break SignInEnd::ListenFailed;
            };
            if params.get("state") != Some(&attempt.state) {
                let _ = reply.send(CallbackReply::NotThis);
                continue;
            }
            let end = self.finish(&attempt, &params).await;
            let _ = reply.send(if end == SignInEnd::SignedIn {
                CallbackReply::SignedIn
            } else {
                CallbackReply::Failed
            });
            break end;
        };
        self.end_waiting(end, Some(&attempt.operation_id));
    }

    /// Everything after the browser came back with this attempt's state.
    async fn finish(
        &self,
        attempt: &Attempt,
        params: &std::collections::BTreeMap<String, String>,
    ) -> SignInEnd {
        if params.contains_key("error") {
            return SignInEnd::Declined;
        }
        let Some(code) = params.get("code").filter(|code| !code.is_empty()) else {
            return SignInEnd::Incomplete;
        };
        let returned = params.get("client_id").filter(|id| !id.is_empty());
        let client_id = match (&attempt.account, returned) {
            (None, Some(issued)) if issued != REGISTRATION_CLIENT => issued.clone(),
            (None, _) => return SignInEnd::Incomplete,
            (Some(account), Some(returned)) if returned != &account.client_id => {
                return SignInEnd::WrongAccount
            }
            (Some(account), _) => account.client_id.clone(),
        };
        let Ok(answer) = self
            .authority
            .exchange(&client_id, code, &attempt.verifier, &attempt.redirect_uri)
            .await
        else {
            return SignInEnd::ExchangeFailed;
        };
        let now = self.clock.unix_milliseconds();
        let Some(id_token) = answer.id_token.as_ref() else {
            return SignInEnd::InvalidIdentity;
        };
        let Ok(identity) = self
            .authority
            .identity(id_token, &client_id, &attempt.nonce, now)
            .await
        else {
            return SignInEnd::InvalidIdentity;
        };
        if attempt
            .account
            .as_ref()
            .is_some_and(|account| account.subject != identity.subject)
        {
            return SignInEnd::WrongAccount;
        }
        if !answer.scopes.iter().any(|scope| scope == PLAN_SCOPE) {
            return SignInEnd::PlanNotEnabled;
        }
        let Some(tokens) = tokens(&answer, now, None) else {
            return SignInEnd::Incomplete;
        };
        let _change = self.changes.lock().await;
        let Ok(kept) = self.read() else {
            return SignInEnd::NotSaved;
        };
        let kept = kept.unwrap_or_default();
        // A new sign-in of an account already kept signs that one in again,
        // so one ChatGPT account is never kept twice.
        let id = match &attempt.account {
            Some(account) => account.id.clone(),
            None => kept
                .accounts()
                .iter()
                .find(|each| each.subject == identity.subject)
                .map_or_else(AccountId::mint, |each| each.id.clone()),
        };
        let account = ChatgptAccount {
            id,
            email: identity
                .email
                .or_else(|| {
                    attempt
                        .account
                        .as_ref()
                        .map(|account| account.email.clone())
                })
                .unwrap_or_default(),
            subject: identity.subject,
            client_id,
            id_token: id_token.clone(),
            tokens: Some(tokens),
        };
        let Ok(next) = kept.with_signed_in(account.clone()) else {
            return SignInEnd::TooManyAccounts;
        };
        if self.write(&next).is_err() {
            return SignInEnd::NotSaved;
        }
        tracing::info!(operation_id = %attempt.operation_id, account = account.id.as_str(), "a ChatGPT account signed in");
        SignInEnd::SignedIn
    }

    /// Codex uses `account`, or the saved API key when `None` (row A2).
    ///
    /// # Errors
    /// [`LoginsError::UnknownAccount`], and the vault's and audit's own.
    pub async fn use_account(
        &self,
        caller: LoginCaller,
        account: Option<AccountId>,
    ) -> Result<(), LoginsError> {
        let _change = self.changes.lock().await;
        let accounts = self.read()?.unwrap_or_default();
        let next = accounts
            .using(account.clone())
            .map_err(|UnknownAccount| LoginsError::UnknownAccount)?;
        if next == accounts {
            return Ok(());
        }
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::ChatgptUseRequested {
                account: account.clone(),
            },
        )?;
        if let Err(error) = self.write(&next) {
            let _ = self.record(
                &operation_id,
                Some(&caller),
                LoginEvent::ChatgptUseFailed { account },
            );
            return Err(error);
        }
        self.record_outcome(
            &operation_id,
            Some(&caller),
            LoginEvent::ChatgptUsed { account },
        )
    }

    /// Forget `account`: its refresh token revoked at OpenAI when it can be
    /// (recorded either way), then removed (row A1).
    ///
    /// # Errors
    /// [`LoginsError::UnknownAccount`], and the vault's and audit's own.
    pub async fn remove(&self, caller: LoginCaller, account: AccountId) -> Result<(), LoginsError> {
        let _change = self.changes.lock().await;
        let accounts = self.read()?.unwrap_or_default();
        let kept = accounts
            .account(&account)
            .cloned()
            .ok_or(LoginsError::UnknownAccount)?;
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            Some(&caller),
            LoginEvent::ChatgptRemoveRequested {
                account: account.clone(),
            },
        )?;
        let revoked = match &kept.tokens {
            Some(tokens) => self
                .authority
                .revoke(&kept.client_id, &tokens.refresh)
                .await
                .is_ok(),
            None => true,
        };
        if let Err(error) = self.write(&accounts.without(&account)) {
            let _ = self.record(
                &operation_id,
                Some(&caller),
                LoginEvent::ChatgptRemoveFailed { account },
            );
            return Err(error);
        }
        self.record_outcome(
            &operation_id,
            Some(&caller),
            LoginEvent::ChatgptRemoved { account, revoked },
        )
    }

    /// An access token of the account Codex uses, for one lease: as it is
    /// while it has [`super::domain::REFRESH_BELOW`] left, else refreshed
    /// and saved first. `None` when no account is in use (rows R1 to R8).
    ///
    /// # Errors
    /// [`AccessRefused`]: nothing is handed.
    pub async fn access(&self) -> Result<Option<ChatgptAccess>, AccessRefused> {
        let _change = self.changes.lock().await;
        let accounts = match self.read() {
            Ok(Some(accounts)) => accounts,
            Ok(None) => return Ok(None),
            Err(LoginsError::VaultLocked) => return Err(AccessRefused::Locked),
            Err(_) => return Err(AccessRefused::Unavailable),
        };
        let Some(account) = accounts.in_use().cloned() else {
            return Ok(None);
        };
        let Some(current) = account.tokens.clone() else {
            return Err(AccessRefused::SignInNeeded);
        };
        let now = self.clock.unix_milliseconds();
        if !current.need_refresh(now) {
            return handed(&account.id, &current).map(Some);
        }
        let operation_id = Uuid::new_v4().to_string();
        let failed = |why: RefreshProblem| {
            if self
                .record(
                    &operation_id,
                    None,
                    LoginEvent::ChatgptRefreshFailed {
                        account: account.id.clone(),
                        why,
                    },
                )
                .is_err()
            {
                tracing::error!(
                    account = account.id.as_str(),
                    "a failed ChatGPT refresh could not be recorded"
                );
            }
            match why {
                RefreshProblem::Unusable => AccessRefused::SignInNeeded,
                other => AccessRefused::Refresh(other),
            }
        };
        let answer = match self
            .authority
            .refresh(&account.client_id, &current.refresh)
            .await
        {
            Ok(answer) => answer,
            Err(RefreshFailure::Unreachable) => return Err(failed(RefreshProblem::Unreachable)),
            Err(RefreshFailure::Unusable) => {
                // Its tokens are dropped, so it reads as needing a sign-in.
                if self
                    .write(&accounts.with_tokens(&account.id, None))
                    .is_err()
                {
                    tracing::error!(
                        account = account.id.as_str(),
                        "an unusable ChatGPT refresh token could not be dropped"
                    );
                }
                return Err(failed(RefreshProblem::Unusable));
            }
        };
        let now = self.clock.unix_milliseconds();
        let Some(next) = tokens(&answer, now, Some(&current.refresh)) else {
            return Err(failed(RefreshProblem::Unreachable));
        };
        if self
            .write(&accounts.with_tokens(&account.id, Some(next.clone())))
            .is_err()
        {
            // The old refresh token is spent: the next lease's refresh is
            // refused, and the account then reads as needing a sign-in.
            return Err(failed(RefreshProblem::NotSaved));
        }
        if self
            .record(
                &operation_id,
                None,
                LoginEvent::ChatgptRefreshed {
                    account: account.id.clone(),
                },
            )
            .is_err()
        {
            tracing::error!(
                account = account.id.as_str(),
                "a ChatGPT refresh could not be recorded"
            );
        }
        handed(&account.id, &next).map(Some)
    }
}

fn handed(account: &AccountId, tokens: &Tokens) -> Result<ChatgptAccess, AccessRefused> {
    Ok(ChatgptAccess {
        account: account.clone(),
        login: Login::new(tokens.access.expose().to_owned())
            .map_err(|_| AccessRefused::Refresh(RefreshProblem::Unreachable))?,
        renew_by_ms: tokens.renew_by_ms(),
    })
}

/// The tokens a token answer at `now_ms` gives; a refresh that names no new
/// refresh token keeps `previous`. `None` with no refresh token at all.
fn tokens(answer: &TokenAnswer, now_ms: u64, previous: Option<&Secret>) -> Option<Tokens> {
    let refresh = answer.refresh.clone().or_else(|| previous.cloned())?;
    let lasts = u64::try_from(answer.expires_in.as_millis()).unwrap_or(u64::MAX);
    Some(Tokens {
        access: answer.access.clone(),
        refresh,
        expires_at_ms: now_ms.saturating_add(lasts),
        earliest_refresh_at_ms: answer.earliest_refresh_at_ms,
    })
}
