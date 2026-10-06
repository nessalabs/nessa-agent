//! What the authorization owner asks of the clock, the store, the audit,
//! and the HTTPS authorization server. The domain never sees these.
use async_trait::async_trait;
use uuid::Uuid;

use super::super::domain::{Deletion, Publication, RemoteObservation, ServerAuth};

/// Milliseconds since the Unix epoch, injected so a deadline does not read
/// the machine clock.
pub trait AuthClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Bytes for a PKCE verifier and a callback state. Failure is a refusal,
/// not a guessed value.
pub trait Entropy: Send + Sync {
    fn bytes(&self, len: usize) -> Result<Vec<u8>, ()>;
}

/// A non-secret record and, separately, the secret material.
#[async_trait]
pub trait AuthorizationRecords: Send + Sync {
    async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure>;
    async fn store(&self, auth: &ServerAuth) -> Result<(), RecordFailure>;
    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure>;
    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<Publication, RecordFailure>;
    async fn delete_secret(&self, server: Uuid) -> Result<Deletion, RecordFailure>;
}

/// Why a record could not be read. A write that may have landed is
/// [`Publication::Unknown`], not this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordFailure {
    Unavailable,
}

/// The private token. `Debug` does not print it.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenMaterial {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub generation: u64,
}

impl std::fmt::Debug for TokenMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenMaterial")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// The access token a session may send, without the refresh token.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedToken {
    pub access_token: String,
    pub generation: u64,
}

impl std::fmt::Debug for AdmittedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdmittedToken")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Why a bearer cannot be sent. The transport adapter names these in the
/// SDK session's error type. This enum does not carry a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionRefusal {
    Unauthorized,
    InsufficientScope,
    Unreachable,
}

/// An authorization event with no secret in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthAuditRecord {
    pub server: Uuid,
    pub action: &'static str,
    pub intent: bool,
    pub generation: u64,
    pub resource: String,
    pub phase: String,
}

#[async_trait]
pub trait AuthorizationAudit: Send + Sync {
    async fn record(&self, record: &AuthAuditRecord) -> Result<(), AuditFailure>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditFailure;

/// One HTTPS call. A redirect is a status, not a followed `Location`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthResponse {
    pub status: u16,
    pub body: String,
    pub www_authenticate: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthCallFailure {
    /// The request was not sent.
    NotSent,
    /// The request was sent and the answer was not retained.
    Lost,
}

#[async_trait]
pub trait OAuthHttp: Send + Sync {
    async fn get(&self, url: &str) -> Result<OAuthResponse, OAuthCallFailure>;
    /// `application/x-www-form-urlencoded`, for the token and revocation endpoints.
    async fn post_form(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure>;
    /// `application/json`, for dynamic client registration (RFC 7591).
    async fn post_json(&self, url: &str, body: &str) -> Result<OAuthResponse, OAuthCallFailure>;
}

/// Closes local sessions of one server name. The owner does not wait for a
/// remote process to exit.
#[async_trait]
pub trait SessionDrain: Send + Sync {
    async fn drain(&self, server_name: &str);
}

/// The configured resource a bearer would be sent to.
pub trait ResourceLookup: Send + Sync {
    fn resource(&self, server: Uuid) -> Option<String>;
}

/// A URL change or a removal the settings owner is about to publish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindingChange {
    ResourceChanged {
        id: Uuid,
        previous_url: String,
        url: String,
    },
    Removed {
        id: Uuid,
        url: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FenceRefusal;

/// Settings asks this before a changed remote URL, or a removed remote,
/// enters the live set. `Err` means the old token is not yet fenced, so the
/// new definition must not be admitted.
#[async_trait]
pub trait AuthorizationHandoff: Send + Sync {
    async fn fence(&self, changes: &[BindingChange]) -> Result<(), FenceRefusal>;
}

/// Tests and a gateway with no authorization owner. A production composition
/// does not use this: it passes the authorization owner.
pub struct PermissiveHandoff;

#[async_trait]
impl AuthorizationHandoff for PermissiveHandoff {
    async fn fence(&self, _changes: &[BindingChange]) -> Result<(), FenceRefusal> {
        Ok(())
    }
}

/// What `mcpServers.authorize` answers. No token is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorizeAnswer {
    NotRequired,
    PendingConsent {
        attempt_id: String,
        consent_url: String,
        deadline_ms: u64,
    },
    Ready {
        generation: u64,
    },
    StoreUnavailable,
    RegistrationUnsupported,
    DiscoveryFailed,
    AuthorizationIncomplete,
    AuditUnavailable,
    Busy,
}

/// What `mcpServers.revoke` answers. Pending work stays pending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevokeAnswer {
    pub settled: bool,
    pub local_drained: bool,
    pub secret_deleted: bool,
    pub remote: Option<RemoteObservation>,
    pub evidence_acknowledged: bool,
}

/// Redacted facts `mcpServers.list` may show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedAuthorization {
    pub phase: &'static str,
    pub generation: u64,
    pub token_expired: bool,
    pub refresh_failing: bool,
    pub scope_required: bool,
    pub remote: Option<RemoteObservation>,
    pub domains_digest: Option<String>,
}

/// The loopback redirect the browser is sent back to.
#[async_trait]
pub trait ConsentCallback: Send + Sync {
    /// Bind `http://127.0.0.1:<port>/mcp-oauth/callback` and accept one
    /// matching call until `deadline_ms`. The response body never contains
    /// the code.
    async fn listen(&self, wait_for: std::time::Duration) -> Result<CallbackBind, ()>;
}

pub struct CallbackBind {
    pub redirect_uri: String,
    pub accepted: tokio::sync::oneshot::Receiver<CallbackQuery>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallbackQuery {
    pub state: String,
    pub code: Option<String>,
    pub denied: bool,
}
