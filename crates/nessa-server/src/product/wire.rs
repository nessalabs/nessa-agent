use nessa_auth::application::{dto::CredentialGrantDto, session::AuthenticatedSession};

pub use super::generated::{
    CredentialIssueParams, CredentialListParams, CredentialListResult, CredentialRevokeParams,
    CredentialRevokeResult, ExistingCredentialResult, IssuedCredentialResult,
    ProductSessionReady as SessionReady, SessionAuthenticateParams, SessionChallenge,
};

use super::generated::PRODUCT_VERSION;
use crate::product_contract::generated::SessionCloseReason;

impl SessionAuthenticateParams {
    pub(crate) fn supports_v1(&self) -> bool {
        supports_product_version(self.min_version, self.max_version)
    }
}

impl SessionReady {
    pub(crate) fn from_session(
        gateway_id: &str,
        session: &AuthenticatedSession,
        grants: Vec<CredentialGrantDto>,
        methods: Vec<String>,
    ) -> Self {
        let context = session.context();
        Self {
            version: PRODUCT_VERSION,
            gateway_id: gateway_id.to_owned(),
            principal_id: context.principal_id().as_str().to_owned(),
            organization_id: context.organization_id().as_str().to_owned(),
            membership_id: context.membership_id().as_str().to_owned(),
            credential_id: context.credential_id().as_str().to_owned(),
            audience_id: context.audience_id().as_str().to_owned(),
            expires_at: session.expires_at(),
            grants,
            methods,
        }
    }
}

/// The current product version owner decides overlap for both handshake directions.
pub(crate) fn supports_product_version(minimum: u64, maximum: u64) -> bool {
    minimum <= PRODUCT_VERSION && PRODUCT_VERSION <= maximum
}

/// Existing handshake refusal-to-close policy, shared by both wire directions.
pub(crate) fn authentication_close_reason(code: &str) -> SessionCloseReason {
    match code {
        "protocol_incompatible" => SessionCloseReason::ProtocolIncompatible,
        "temporarily_unavailable" => SessionCloseReason::TemporaryUnavailable,
        "handshake_timeout" => SessionCloseReason::HandshakeTimeout,
        _ => SessionCloseReason::AuthenticationFailed,
    }
}
