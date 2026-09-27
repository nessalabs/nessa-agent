use super::CliError;
pub(crate) const MAX_SAFE_TIMESTAMP: u64 = 9_007_199_254_740_991;
/// Verified identity returned by the gateway handshake, not command arguments.
pub struct GatewayIdentity {
    pub gateway_id: String,
    pub organization_id: String,
    pub principal_id: String,
    pub expires_at: Option<u64>,
}

/// A new browser principal with only chat and health access on this gateway.
pub struct TokenRequest {
    pub request_id: String,
    pub principal_id: String,
    pub membership_id: String,
    pub gateway_id: String,
    pub organization_id: String,
    pub expires_at: Option<u64>,
}

/// Secret delivered once by the server. Never implement Debug or log its contents.
pub struct BrowserToken {
    pub credential_id: String,
    pub secret: String,
}

/// The CLI consumes authenticated gateway operations, never local registry storage.
pub trait Gateway {
    fn identity(&self) -> &GatewayIdentity;
    fn health(&mut self) -> Result<(), CliError>;
    fn issue(&mut self, request: TokenRequest) -> Result<BrowserToken, CliError>;
}

/// Request a browser credential with an optional TTL, capped by the caller's expiry.
/// IDs and time come from composition; only the server authorizes issuance.
pub fn issue_token(
    gateway: &mut dyn Gateway,
    request_id: String,
    principal_id: String,
    membership_id: String,
    now: u64,
    ttl_seconds: Option<u64>,
) -> Result<BrowserToken, CliError> {
    let identity = gateway.identity();
    if identity
        .expires_at
        .is_some_and(|expiry| expiry > MAX_SAFE_TIMESTAMP)
    {
        return Err(CliError::InvalidLifetime);
    }
    let requested = ttl_seconds
        .map(|ttl| {
            now.checked_add(ttl)
                .filter(|expiry| ttl > 0 && *expiry <= MAX_SAFE_TIMESTAMP)
                .ok_or(CliError::InvalidLifetime)
        })
        .transpose()?;
    let expires_at = requested.into_iter().chain(identity.expires_at).min();
    if expires_at.is_some_and(|expiry| expiry <= now) {
        return Err(CliError::Expired);
    }
    let request = TokenRequest {
        request_id,
        principal_id,
        membership_id,
        gateway_id: identity.gateway_id.clone(),
        organization_id: identity.organization_id.clone(),
        expires_at,
    };
    gateway.issue(request)
}
