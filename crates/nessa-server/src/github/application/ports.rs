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

/// Where the one sign-in is kept between runs.
#[async_trait]
pub trait GithubTokenStore: Send + Sync {
    async fn load(&self) -> Result<Option<GithubSignIn>, StoreUnavailable>;
    async fn save(&self, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable>;
    /// Deleting what is not there succeeds.
    async fn delete(&self) -> Result<(), StoreUnavailable>;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GithubAuditEvent {
    /// Before GitHub is asked for a code. When it cannot be kept, nothing is
    /// asked.
    ConnectRequested,
    /// Approved, before the token is kept. When it cannot be kept, the token
    /// is dropped.
    Connected { account: GithubAccount },
    /// The attempt ended without a sign-in, other than by a cancel.
    ConnectEnded(ConnectEnd),
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
}
