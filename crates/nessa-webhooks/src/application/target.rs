//! Who a committed registration delivers as.
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};

/// Secret-free identity of a committed registration. Infrastructure supplies
/// this from its private registration row, never from an event payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub credential: CredentialId,
    pub principal: PrincipalId,
    pub organization: OrganizationId,
    pub owner: PrincipalId,
}
