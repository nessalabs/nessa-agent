//! Typed projections of the Cedar schema's entity attributes.
//!
//! These adapter-owned records preserve domain ID and role types until conversion
//! to Cedar expressions. Only `into_cedar` methods know Cedar's attribute keys.

use crate::{
    application::ports::{AccessError, AccessSnapshot, IssuePath},
    domain::{Grant, MembershipRole, OrganizationId, PrincipalKind, Resource},
};
use cedar_policy::RestrictedExpression;
use std::collections::HashMap;

/// Attributes of a Nessa::Actor, projected from its current membership and the
/// grants its credential holds. The grants are facts; whether one covers a
/// request is the policy's question, not this adapter's.
pub(super) struct ActorAttributes<'a> {
    organization_id: &'a OrganizationId,
    role: MembershipRole,
    active: bool,
    grants: &'a [Grant],
    issuance: crate::domain::IssuanceCause,
}

impl<'a> From<&'a AccessSnapshot> for ActorAttributes<'a> {
    fn from(snapshot: &'a AccessSnapshot) -> Self {
        Self {
            organization_id: snapshot.membership.organization_id(),
            role: snapshot.membership.role(),
            active: snapshot.membership.is_active(),
            grants: snapshot.credential.grants(),
            issuance: snapshot.origin.cause(),
        }
    }
}

impl ActorAttributes<'_> {
    /// Encode exactly the fields and value types declared for Actor in schema.json.
    pub(super) fn into_cedar(self) -> Result<HashMap<String, RestrictedExpression>, AccessError> {
        let role = role(self.role);
        let grants = self
            .grants
            .iter()
            .map(grant)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HashMap::from([
            (
                "organizationId".to_owned(),
                RestrictedExpression::new_string(self.organization_id.as_str().to_owned()),
            ),
            (
                "role".to_owned(),
                RestrictedExpression::new_string(role.to_owned()),
            ),
            (
                "active".to_owned(),
                RestrictedExpression::new_bool(self.active),
            ),
            ("grants".to_owned(), RestrictedExpression::new_set(grants)),
            (
                "issuance".to_owned(),
                RestrictedExpression::new_string(
                    match self.issuance {
                        crate::domain::IssuanceCause::Bootstrap => "bootstrap",
                        crate::domain::IssuanceCause::AdminIssue => "admin_issue",
                        crate::domain::IssuanceCause::WebhookIssue => "webhook_issue",
                        crate::domain::IssuanceCause::DevicePairing => "device_pairing",
                        crate::domain::IssuanceCause::SurfaceProvision => "surface_provision",
                        crate::domain::IssuanceCause::OwnerRecovery => "owner_recovery",
                    }
                    .to_owned(),
                ),
            ),
        ]))
    }
}

/// One credential grant as the schema's `Grant` record.
fn grant(grant: &Grant) -> Result<RestrictedExpression, AccessError> {
    let string = |value: &str| RestrictedExpression::new_string(value.to_owned());
    RestrictedExpression::new_record([
        ("action".to_owned(), string(grant.action().as_str())),
        (
            "resourceId".to_owned(),
            string(grant.resource().id().as_str()),
        ),
        (
            "organizationId".to_owned(),
            string(grant.resource().organization_id().as_str()),
        ),
    ])
    .map_err(|_| AccessError::Unavailable)
}

/// Attributes of a Nessa::Resource, projected from the authoritative resource.
pub(super) struct ResourceAttributes<'a> {
    resource: &'a Resource,
}

impl<'a> From<&'a Resource> for ResourceAttributes<'a> {
    fn from(resource: &'a Resource) -> Self {
        Self { resource }
    }
}

impl ResourceAttributes<'_> {
    /// Encode exactly the fields and value types declared for Resource in schema.json.
    pub(super) fn into_cedar(self) -> HashMap<String, RestrictedExpression> {
        HashMap::from([
            (
                "id".to_owned(),
                RestrictedExpression::new_string(self.resource.id().as_str().to_owned()),
            ),
            (
                "organizationId".to_owned(),
                RestrictedExpression::new_string(
                    self.resource.organization_id().as_str().to_owned(),
                ),
            ),
        ])
    }
}

/// A membership role as the policies spell it.
pub(super) fn role(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Admin => "admin",
        MembershipRole::Member => "member",
    }
}

/// A principal kind as the policies spell it.
pub(super) fn kind(kind: PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent => "agent",
        PrincipalKind::Integration => "integration",
        PrincipalKind::Gateway => "gateway",
    }
}

/// An issuing path as the policies spell it in `context.path`.
pub(super) fn path(path: IssuePath) -> &'static str {
    match path {
        IssuePath::Bootstrap => "bootstrap",
        IssuePath::ProductIssue => "product",
        IssuePath::WebhookIssue => "webhook",
        IssuePath::SurfaceProvision => "provision",
        IssuePath::OwnerRecovery => "recovery",
        IssuePath::StoredOnLoad => "stored",
    }
}
