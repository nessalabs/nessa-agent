//! What the ChatGPT sign-in asks of OpenAI and of the computer the browser
//! comes back to. The domain never sees these.
use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::domain::Secret;

/// What a sign-in's browser is sent to OpenAI with.
#[derive(Debug)]
pub struct Authorize<'a> {
    /// [`super::domain::REGISTRATION_CLIENT`] for a new account, else the
    /// account's issued client ID.
    pub client_id: &'a str,
    /// Whether this registers a new account (`agent_name_hint` is sent).
    pub register: bool,
    pub host_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub nonce: &'a str,
    /// The PKCE challenge (S256).
    pub challenge: &'a str,
    /// A saved account's last ID token, signing it in again.
    pub id_token_hint: Option<&'a Secret>,
    pub login_hint: Option<&'a str>,
}

/// OpenAI's token endpoint's answer.
#[derive(Debug)]
pub struct TokenAnswer {
    pub access: Secret,
    /// Absent only when `offline_access` was not granted.
    pub refresh: Option<Secret>,
    pub id_token: Option<Secret>,
    pub expires_in: Duration,
    pub earliest_refresh_at_ms: Option<u64>,
    pub scopes: Vec<String>,
}

/// A call to OpenAI did not give what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityFailure {
    /// No answer: the network, a timeout, or OpenAI's own failure (5xx).
    Unreachable,
    /// OpenAI refused it.
    Refused,
    /// An answer this adapter cannot read, or an ID token that does not
    /// verify.
    Malformed,
}

/// A refresh did not give new tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshFailure {
    /// The refresh token or the client is no longer usable
    /// (`invalid_grant`, `refresh_token_reused`, `invalid_client`…).
    Unusable,
    /// No answer, or one this adapter cannot read: the tokens are kept.
    Unreachable,
}

/// Who an ID token says signed in, once verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
}

/// OpenAI's sign-in, at the addresses the adapter was built for.
#[async_trait]
pub trait ChatgptAuthority: Send + Sync {
    /// Where the browser is sent.
    fn authorize_url(&self, request: &Authorize<'_>) -> String;
    async fn exchange(
        &self,
        client_id: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<TokenAnswer, AuthorityFailure>;
    async fn refresh(
        &self,
        client_id: &str,
        refresh: &Secret,
    ) -> Result<TokenAnswer, RefreshFailure>;
    async fn revoke(&self, client_id: &str, refresh: &Secret) -> Result<(), AuthorityFailure>;
    /// Verify `id_token`'s signature against OpenAI's published keys, and
    /// its issuer, audience (`client_id`), expiry against `now_ms`, and
    /// `nonce`.
    async fn identity(
        &self,
        id_token: &Secret,
        client_id: &str,
        nonce: &str,
        now_ms: u64,
    ) -> Result<Identity, AuthorityFailure>;
}

/// What the browser was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackReply {
    /// Signed in: the tab can be closed.
    SignedIn,
    /// This attempt ended without a sign-in.
    Failed,
    /// Not this attempt's (its `state` differs): ignored.
    NotThis,
}

/// One request the browser made to the callback address.
#[derive(Debug)]
pub struct Callback {
    pub params: BTreeMap<String, String>,
    pub reply: oneshot::Sender<CallbackReply>,
}

/// No address on this computer could be listened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenFailed;

/// Where the browser comes back to: one address for one attempt.
#[async_trait]
pub trait CallbackServer: Send {
    fn redirect_uri(&self) -> String;
    /// The next request to the callback path; `None` once it stops.
    async fn next(&mut self) -> Option<Callback>;
}

#[async_trait]
pub trait CallbackListener: Send + Sync {
    async fn listen(&self) -> Result<Box<dyn CallbackServer>, ListenFailed>;
}
