//! Typed projections of the Cedar schema's entity attributes.
//!
//! These adapter-owned records preserve domain ID and role types until conversion
//! to Cedar expressions. Only `into_cedar` methods know Cedar's attribute keys.

use crate::{
    application::ports::{AccessError, AccessSnapshot},
    domain::{Grant, MembershipRole, OrganizationId, Resource},
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
}

impl<'a> From<&'a AccessSnapshot> for ActorAttributes<'a> {
    fn from(snapshot: &'a AccessSnapshot) -> Self {
        Self {
            organization_id: snapshot.membership.organization_id(),
            role: snapshot.membership.role(),
            active: snapshot.membership.is_active(),
            grants: snapshot.credential.grants(),
        }
    }
}

impl ActorAttributes<'_> {
    /// Encode exactly the fields and value types declared for Actor in schema.json.
    pub(super) fn into_cedar(self) -> Result<HashMap<String, RestrictedExpression>, AccessError> {
        let role = match self.role {
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
        };
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
