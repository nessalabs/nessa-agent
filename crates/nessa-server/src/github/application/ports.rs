//! What the sign-in owner asks of GitHub, the store and the audit. The
//! domain never sees these.
use std::time::Duration;

use async_trait::async_trait;

use super::super::domain::{AccessToken, ConnectEnd, GithubAccount, GithubSignIn};

/// The secret half of a device authorization, sent back with each poll.
/// `Debug` does not print it.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceCode(pub String);

impl std::fmt::Debug for DeviceCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeviceCode(..)")
    }
}

/// GitHub's answer to a request for a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAuthorization {
    pub device_code: DeviceCode,
    /// What the person types on GitHub.
    pub user_code: String,
    /// Where they type it.
    pub verification_uri: String,
    /// How long the code lasts from now.
    pub expires_in: Duration,
    /// How long to wait between polls.
    pub interval: Duration,
}

/// GitHub's answer to one poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollAnswer {
    /// The person has not answered yet.
    Pending,
    /// Polling too fast: wait `interval` from now on.
    SlowDown { interval: Duration },
    /// Approved.
    Granted(AccessToken),
    /// The code's time ran out.
    Expired,
    /// The person refused.
    Denied,
}

/// Why a call to GitHub did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiFailure {
    /// No answer: the network, a timeout, or GitHub's own failure (5xx).
    Unreachable,
    /// GitHub refused the app: an unknown client id, or the device flow is
    /// off for it.
    Rejected,
    /// GitHub refused the token.
    Unauthorized,
    /// An answer this adapter cannot read.
    Malformed,
}

/// The three calls of a device-flow sign-in, for the Nessa GitHub App the
/// adapter was built for.
#[async_trait]
pub trait GithubApi: Send + Sync {
    async fn request_device_code(&self) -> Result<DeviceAuthorization, ApiFailure>;
    async fn poll(&self, device_code: &DeviceCode) -> Result<PollAnswer, ApiFailure>;
    /// The account `token` signs in as.
    async fn account(&self, token: &AccessToken) -> Result<GithubAccount, ApiFailure>;
}

/// The store could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreUnavailable;

/// Where the one sign-in is kept between runs. Every operation carries the
/// unlock generation that admitted it; a stale generation is StoreUnavailable.
#[async_trait]
pub trait GithubTokenStore: Send + Sync {
    async fn load(&self, generation: u64) -> Result<Option<GithubSignIn>, StoreUnavailable>;
    async fn save(&self, generation: u64, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable>;
    /// Within the captured generation, deleting an absent entry succeeds.
    async fn delete(&self, generation: u64) -> Result<(), StoreUnavailable>;
}

/// Who asked: the authenticated session's identity, as the audit names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubCaller {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// One record of a sign-in or a sign-out. Never a token or a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubAuditRecord {
    /// Shared by an operation's requested record and its outcome.
    pub operation_id: String,
    /// Who started it. A sign-in finishing later is still theirs.
    pub caller: GithubCaller,
    pub event: GithubAuditEvent,
}

/// What decided that an attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndCause {
    /// GitHub answered: declined, expired, or the app refused.
    GithubAnswered,
    /// The code's time ran out here with no answer that settled it.
    DeadlinePassed,
    /// GitHub could not be reached, or answered nothing usable, when asked
    /// for a code.
    GithubUnreachable,
    /// GitHub gave a sign-in that the gateway could not keep.
    StoreFailed,
    /// The local vault session changed before GitHub answered.
    VaultLocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GithubAuditEvent {
    /// Before GitHub is asked for a code. When it cannot be kept, nothing is
    /// asked.
    ConnectRequested,
    /// Approved, before the token is kept. When it cannot be kept, the token
    /// is dropped.
    Connected { account: GithubAccount },
    /// The attempt ended without a sign-in, other than by a cancel, and what
    /// decided it.
    ConnectEnded { end: ConnectEnd, cause: EndCause },
    /// The attempt was cancelled, by `by`, who may not be who started it.
    ConnectCancelled { by: GithubCaller },
    /// Before the token is deleted. When it cannot be kept, nothing is
    /// deleted.
    DisconnectRequested { account: GithubAccount },
    /// The token is deleted.
    Disconnected,
    /// The token could not be deleted and is still kept.
    DisconnectFailed,
}

/// The record could not be made durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditUnavailable;

#[async_trait]
pub trait GithubAudit: Send + Sync {
    async fn record(&self, record: &GithubAuditRecord) -> Result<(), AuditUnavailable>;
    /// Blocking durable delivery when synchronous lifetime cleanup cannot await.
    /// Uses the same record identity/writer as ordinary delivery; failures remain visible.
    fn record_cleanup(&self, record: &GithubAuditRecord) -> Result<(), AuditUnavailable>;
}

/// Cryptographic availability of the sign-in vault, independent of caller policy.
pub trait GithubAccess: Send + Sync {
    fn generation(&self) -> Option<u64>;
}

/// Generation-scoped bytes for the infrastructure sign-in codec. The vault
/// compares generation atomically with key access; this port grants no caller policy.
pub trait GithubSecret: Send + Sync {
    fn read(
        &self,
        generation: u64,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, StoreUnavailable>;
    fn write(&self, generation: u64, bytes: &[u8]) -> Result<(), StoreUnavailable>;
    fn delete(&self, generation: u64) -> Result<(), StoreUnavailable>;
}
