//! Typed projections of the Cedar schema's entity attributes.
//!
//! These adapter-owned records preserve domain ID and role types until conversion
//! to Cedar expressions. Only `into_cedar` methods know Cedar's attribute keys.

use crate::domain::{Membership, MembershipRole, OrganizationId, Resource};
use cedar_policy::RestrictedExpression;
use std::collections::HashMap;

/// Attributes of a Nessa::Actor, projected from its current membership.
pub(super) struct ActorAttributes<'a> {
    organization_id: &'a OrganizationId,
    role: MembershipRole,
    active: bool,
}

impl<'a> From<&'a Membership> for ActorAttributes<'a> {
    fn from(membership: &'a Membership) -> Self {
        Self {
            organization_id: membership.organization_id(),
            role: membership.role(),
            active: membership.is_active(),
        }
    }
}

impl ActorAttributes<'_> {
    /// Encode exactly the fields and value types declared for Actor in schema.json.
    pub(super) fn into_cedar(self) -> HashMap<String, RestrictedExpression> {
        let role = match self.role {
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
        };
        HashMap::from([
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
        ])
    }
}

/// Attributes of a Nessa::Resource, projected from the authoritative resource.
pub(super) struct ResourceAttributes<'a> {
    organization_id: &'a OrganizationId,
}

impl<'a> From<&'a Resource> for ResourceAttributes<'a> {
    fn from(resource: &'a Resource) -> Self {
        Self {
            organization_id: resource.organization_id(),
        }
    }
}

impl ResourceAttributes<'_> {
    /// Encode exactly the fields and value types declared for Resource in schema.json.
    pub(super) fn into_cedar(self) -> HashMap<String, RestrictedExpression> {
        HashMap::from([(
            "organizationId".to_owned(),
            RestrictedExpression::new_string(self.organization_id.as_str().to_owned()),
        )])
    }
}
