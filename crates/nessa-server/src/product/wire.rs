use nessa_auth::application::{dto::CredentialGrantDto, session::AuthenticatedSession};

pub use super::generated::{
    CredentialIssueParams, CredentialListParams, CredentialListResult, CredentialRevokeParams,
    CredentialRevokeResult, ExistingCredentialResult, IssuedCredentialResult,
    ProductSessionReady as SessionReady, SessionAuthenticateParams, SessionChallenge,
};

pub const PRODUCT_VERSION: u64 = 1;

impl SessionAuthenticateParams {
    pub(crate) fn supports_v1(&self) -> bool {
        self.min_version <= PRODUCT_VERSION && PRODUCT_VERSION <= self.max_version
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
