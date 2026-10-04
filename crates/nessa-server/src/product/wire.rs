use nessa_auth::application::{dto::CredentialGrantDto, session::AuthenticatedSession};

use nessa_protocol::product::generated::{ProductSessionReady, PRODUCT_VERSION};

/// The ready frame for an authenticated session: what the gateway tells a
/// client about who it is, what it may do, and until when.
pub(crate) fn ready_frame(
    gateway_id: &str,
    session: &AuthenticatedSession,
    grants: Vec<CredentialGrantDto>,
    methods: Vec<String>,
) -> ProductSessionReady {
    let context = session.context();
    ProductSessionReady {
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
