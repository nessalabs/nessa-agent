//! What a remote session asks before it sends a bearer, and what it does when
//! the server rejects one. The gateway's transport adapter implements this;
//! the SDK does not discover, store, or refresh tokens.
//!
//! ```text
//! HttpSession ──bearer(server id)──▶ RemoteAuthorization
//!            ◀──one token or none───┘
//!            ──rejected(challenge)─▶ one retry token, or a typed refusal
//! ```
//!
//! Arrows are calls. `bearer` and `rejected` may run concurrently for one
//! server; the implementor single-flights refresh. A token is never written
//! to a log by this type's `Debug`.
#![deny(missing_docs)]

use async_trait::async_trait;
use std::fmt;
use uuid::Uuid;

use super::McpError;

/// An opaque access token and the generation the authorization owner published.
///
/// `Debug` prints the generation only (`a_bearer_debug_omits_the_token`).
#[derive(Clone)]
pub struct Bearer {
    token: String,
    generation: u64,
}

impl Bearer {
    /// `token` at `generation`. The caller keeps the only copy it needs; this
    /// value does not zero the string on drop.
    pub fn new(token: impl Into<String>, generation: u64) -> Self {
        Self {
            token: token.into(),
            generation,
        }
    }

    /// The opaque token. Callers put it on one request and do not log it.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The generation the owner published this token under.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl fmt::Debug for Bearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bearer")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Supplies a bearer for one remote server, or refuses with a typed error.
///
/// Implementors are `Send + Sync`. Concurrent `bearer` calls for one server
/// share one refresh. `rejected` is the only retry path: the transport calls
/// it once after HTTP 401 and, when it returns a token, sends the original
/// request once more. It does not call `rejected` after HTTP 403.
#[async_trait]
pub trait RemoteAuthorization: Send + Sync {
    /// The bearer to send for `server`, or `None` when the server needs none.
    ///
    /// # Errors
    ///
    /// [`McpError::Unauthorized`] when consent is required or the token cannot
    /// be used, [`McpError::InsufficientScope`] when a broader scope is
    /// required, and [`McpError::Unreachable`] when the token cannot be read.
    async fn bearer(&self, server: Uuid) -> Result<Option<Bearer>, McpError>;

    /// `server` answered 401 with `www_authenticate`.
    ///
    /// Return a replacement bearer to retry the refused request once. Return
    /// an error to stop: the transport does not send the request again.
    ///
    /// # Errors
    ///
    /// [`McpError::Unauthorized`], [`McpError::InsufficientScope`], or
    /// [`McpError::Unreachable`], as [`Self::bearer`] does.
    async fn rejected(
        &self,
        server: Uuid,
        www_authenticate: &str,
    ) -> Result<Option<Bearer>, McpError>;

    /// `server` answered 403 `insufficient_scope`. The transport does not
    /// retry the request. The owner records that a new consent is required.
    async fn insufficient_scope(&self, server: Uuid, www_authenticate: &str);
}

/// No tokens. A 401 stays unauthorized. Used when a remote server is reached
/// without an authorization owner.
pub struct NoAuthorization;

#[async_trait]
impl RemoteAuthorization for NoAuthorization {
    async fn bearer(&self, _server: Uuid) -> Result<Option<Bearer>, McpError> {
        Ok(None)
    }

    async fn rejected(
        &self,
        _server: Uuid,
        _www_authenticate: &str,
    ) -> Result<Option<Bearer>, McpError> {
        Err(McpError::Unauthorized)
    }

    async fn insufficient_scope(&self, _server: Uuid, _www_authenticate: &str) {}
}
