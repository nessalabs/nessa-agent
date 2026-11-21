//! The ChatGPT accounts Codex can use on other machines, as the gateway keeps
//! them in one vault entry, and the rules for when an access token is handed
//! as it is, refreshed, or replaced (`docs/design/chatgpt-sign-in.md`).
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroize;

/// The most ChatGPT accounts the gateway keeps. `LoginsChatgpt.accounts`
/// in the product schema publishes it, and protocol generation fails if the
/// two differ.
pub const MAX_ACCOUNTS: usize = 8;
/// A token with less than this left is refreshed before it is handed, so a
/// machine gets one with at least this long to run.
pub const REFRESH_BELOW: Duration = Duration::from_secs(50 * 60);
/// How long before a handed token expires its agent is parked when idle, so
/// the next message reopens it with a fresh one.
pub const RENEW_BEFORE: Duration = Duration::from_secs(10 * 60);
/// How long a sign-in waits for the browser to come back.
pub const SIGN_IN_WAIT: Duration = Duration::from_secs(10 * 60);
/// The scope that lets an access token use the person's ChatGPT plan.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// Every scope a sign-in asks for: identity, a refresh token, and plan use.
pub const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The API the tokens are for.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// The client ID a first sign-in of an account registers through. Never
/// kept, and never used for an exchange or a refresh.
pub const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
/// The name the person approves in ChatGPT, the same on every install.
pub const AGENT_NAME: &str = "Nessa";

/// A token, never printed and wiped when dropped.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Secret(redacted)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// An account's identity in this gateway: minted when it is first signed
/// in, kept across later sign-ins and refreshes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AccountId(String);

/// Not an account id: a UUID in its hyphenated lowercase form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidAccountId;

impl AccountId {
    pub fn mint() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    /// # Errors
    /// [`InvalidAccountId`] for anything but a hyphenated lowercase UUID.
    pub fn parse(value: &str) -> Result<Self, InvalidAccountId> {
        match uuid::Uuid::parse_str(value) {
            Ok(id) if id.to_string() == value => Ok(Self(value.to_owned())),
            _ => Err(InvalidAccountId),
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AccountId {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).map_err(|_| "not an account id")
    }
}

impl From<AccountId> for String {
    fn from(id: AccountId) -> Self {
        id.0
    }
}

/// An account's current access and refresh tokens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tokens {
    pub access: Secret,
    pub refresh: Secret,
    pub expires_at_ms: u64,
    /// OpenAI's `earliest_refresh_at`: no refresh before it, unless the
    /// access token has expired.
    pub earliest_refresh_at_ms: Option<u64>,
}

impl Tokens {
    /// Whether these are refreshed before they are handed at `now_ms`.
    pub fn need_refresh(&self, now_ms: u64) -> bool {
        if self.expires_at_ms <= now_ms {
            return true;
        }
        let left = self.expires_at_ms - now_ms;
        left < millis(REFRESH_BELOW) && self.earliest_refresh_at_ms.is_none_or(|at| now_ms >= at)
    }
    /// When an agent started with these should be started again.
    pub fn renew_by_ms(&self) -> u64 {
        self.expires_at_ms.saturating_sub(millis(RENEW_BEFORE))
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// One ChatGPT account: who it is, the client ID OpenAI issued for it, and
/// its tokens. `tokens` is `None` once OpenAI refused its refresh token:
/// it needs signing in again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatgptAccount {
    pub id: AccountId,
    pub email: String,
    /// The ID token's verified `sub`.
    pub subject: String,
    pub client_id: String,
    /// The last ID token, sent as `id_token_hint` when signing in again.
    pub id_token: Secret,
    pub tokens: Option<Tokens>,
}

/// More accounts than [`MAX_ACCOUNTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooManyAccounts;

/// No account with that id is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownAccount;

/// The bytes of the vault entry are not accounts this build can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// Everything the vault entry `login.codex.chatgpt` holds: this gateway's
/// host ID, the accounts, and which one Codex uses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatgptAccounts {
    /// This gateway's `ext_agent_host_id`, made before its first sign-in
    /// and sent with every one after.
    host_id: String,
    in_use: Option<AccountId>,
    accounts: Vec<ChatgptAccount>,
}

impl ChatgptAccounts {
    /// None yet, with a new host ID.
    pub fn new() -> Self {
        Self {
            host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            in_use: None,
            accounts: Vec::new(),
        }
    }

    /// # Errors
    /// [`Unreadable`] for anything [`Self::encode`] did not write, or more
    /// accounts than [`MAX_ACCOUNTS`], or one in use that is not kept.
    pub fn decode(bytes: &[u8]) -> Result<Self, Unreadable> {
        let accounts: Self = serde_json::from_slice(bytes).map_err(|_| Unreadable)?;
        let known = accounts
            .in_use
            .as_ref()
            .is_none_or(|id| accounts.account(id).is_some());
        if accounts.accounts.len() > MAX_ACCOUNTS || !known || accounts.host_id.is_empty() {
            return Err(Unreadable);
        }
        Ok(accounts)
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn accounts(&self) -> &[ChatgptAccount] {
        &self.accounts
    }

    pub fn account(&self, id: &AccountId) -> Option<&ChatgptAccount> {
        self.accounts.iter().find(|account| &account.id == id)
    }

    /// The account Codex uses, if one is chosen.
    pub fn in_use(&self) -> Option<&ChatgptAccount> {
        self.in_use.as_ref().and_then(|id| self.account(id))
    }

    /// These accounts with `account` kept: replacing the one with its id,
    /// or added. The first account kept becomes the one Codex uses.
    ///
    /// # Errors
    /// [`TooManyAccounts`] when adding one passes [`MAX_ACCOUNTS`].
    pub fn with_signed_in(&self, account: ChatgptAccount) -> Result<Self, TooManyAccounts> {
        let mut accounts = self.accounts.clone();
        match accounts.iter().position(|each| each.id == account.id) {
            Some(index) => accounts[index] = account.clone(),
            None if accounts.len() >= MAX_ACCOUNTS => return Err(TooManyAccounts),
            None => accounts.push(account.clone()),
        }
        Ok(Self {
            host_id: self.host_id.clone(),
            in_use: self.in_use.clone().or(Some(account.id)),
            accounts,
        })
    }

    /// These accounts with Codex using `id`, or none of them.
    ///
    /// # Errors
    /// [`UnknownAccount`] when `id` is not kept.
    pub fn using(&self, id: Option<AccountId>) -> Result<Self, UnknownAccount> {
        if id.as_ref().is_some_and(|id| self.account(id).is_none()) {
            return Err(UnknownAccount);
        }
        Ok(Self {
            in_use: id,
            ..self.clone()
        })
    }

    /// These accounts without `id`; Codex uses none if it used that one.
    pub fn without(&self, id: &AccountId) -> Self {
        Self {
            host_id: self.host_id.clone(),
            in_use: self.in_use.clone().filter(|used| used != id),
            accounts: self
                .accounts
                .iter()
                .filter(|account| &account.id != id)
                .cloned()
                .collect(),
        }
    }

    /// These accounts with `id`'s tokens replaced; `None` marks it as
    /// needing a sign-in. Unchanged when `id` is not kept.
    pub fn with_tokens(&self, id: &AccountId, tokens: Option<Tokens>) -> Self {
        Self {
            accounts: self
                .accounts
                .iter()
                .map(|account| {
                    if &account.id == id {
                        ChatgptAccount {
                            tokens: tokens.clone(),
                            ..account.clone()
                        }
                    } else {
                        account.clone()
                    }
                })
                .collect(),
            ..self.clone()
        }
    }
}

impl Default for ChatgptAccounts {
    fn default() -> Self {
        Self::new()
    }
}

/// How the last sign-in ended. Published as `ChatgptSignInEnd` in the
/// product schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInEnd {
    SignedIn,
    /// The person declined in the browser, or OpenAI answered an error.
    Declined,
    /// The browser came back without what a sign-in needs (a code, or a
    /// new registration's client ID), or the token answer had no refresh
    /// token.
    Incomplete,
    /// The sign-in was for another account than the one asked to sign in
    /// again.
    WrongAccount,
    /// OpenAI refused the code, or could not be reached.
    ExchangeFailed,
    /// The ID token did not verify.
    InvalidIdentity,
    /// The person did not let Nessa use their ChatGPT plan.
    PlanNotEnabled,
    /// More accounts than [`MAX_ACCOUNTS`].
    TooManyAccounts,
    /// The account could not be saved.
    NotSaved,
    /// The browser did not come back in [`SIGN_IN_WAIT`].
    TimedOut,
    Cancelled,
    /// Another sign-in was started in its place.
    Replaced,
    /// No address on this computer could be listened on for the browser.
    ListenFailed,
}

impl SignInEnd {
    pub fn name(self) -> &'static str {
        match self {
            Self::SignedIn => "signed_in",
            Self::Declined => "declined",
            Self::Incomplete => "incomplete",
            Self::WrongAccount => "wrong_account",
            Self::ExchangeFailed => "exchange_failed",
            Self::InvalidIdentity => "invalid_identity",
            Self::PlanNotEnabled => "plan_not_enabled",
            Self::TooManyAccounts => "too_many_accounts",
            Self::NotSaved => "not_saved",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Replaced => "replaced",
            Self::ListenFailed => "listen_failed",
        }
    }
}

/// Why a refresh handed nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshProblem {
    /// OpenAI refused the refresh token or the client: the account needs a
    /// sign-in again.
    Unusable,
    /// OpenAI could not be reached, or answered nothing this reads. The
    /// tokens are kept.
    Unreachable,
    /// The new tokens could not be saved, and the old refresh token is spent.
    NotSaved,
}

impl RefreshProblem {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unusable => "unusable",
            Self::Unreachable => "unreachable",
            Self::NotSaved => "not_saved",
        }
    }
}
