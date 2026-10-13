//! Translate typed Nessa values to the checked-in Cedar schema.
//!
//! Attribute keys are Cedar record field names. Organization IDs and roles become
//! schema String values here; membership activity remains a Boolean. These wire
//! representations never replace the domain identifiers or MembershipRole enum.

use crate::{
    application::ports::{AccessError, AccessSnapshot},
    domain::{AuthContext, MembershipRole, PrincipalKind, Resource},
};
use cedar_policy::{
    Entities, Entity, EntityId, EntityTypeName, EntityUid, RestrictedExpression, Schema,
};
use std::{collections::HashMap, str::FromStr};

use super::attributes::{self, ActorAttributes, ResourceAttributes};

pub(super) fn cedar_uid(entity_type: &str, id: &str) -> Result<EntityUid, AccessError> {
    let entity_type =
        EntityTypeName::from_str(entity_type).map_err(|_| AccessError::Unavailable)?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

pub(super) fn entities(
    schema: &Schema,
    context: &AuthContext,
    resource: &Resource,
    snapshot: &AccessSnapshot,
) -> Result<Entities, AccessError> {
    let actor = Entity::new(
        cedar_uid("Nessa::Actor", context.principal_id().as_str())?,
        ActorAttributes::from(snapshot).into_cedar()?,
        Default::default(),
    )
    .map_err(|_| AccessError::Unavailable)?;
    let resource = Entity::new(
        cedar_uid("Nessa::Resource", resource.id().as_str())?,
        ResourceAttributes::from(resource).into_cedar(),
        Default::default(),
    )
    .map_err(|_| AccessError::Unavailable)?;
    Entities::from_entities([actor, resource], Some(schema)).map_err(|_| AccessError::Unavailable)
}

/// A `Nessa::Holder` of `kind` and the resource it would hold a grant on:
/// what [`GrantPolicyEvaluator::may_hold`] asks about.
///
/// [`GrantPolicyEvaluator::may_hold`]: crate::application::ports::GrantPolicyEvaluator::may_hold
pub(super) fn holding(
    schema: &Schema,
    kind: PrincipalKind,
    resource: &Resource,
) -> Result<(EntityUid, EntityUid, Entities), AccessError> {
    let kind = attributes::kind(kind);
    principal_and_resource(
        schema,
        cedar_uid("Nessa::Holder", kind)?,
        [("kind", kind)],
        resource,
    )
}

/// A `Nessa::Recipient`, the principal kind and membership role a new
/// credential would be issued to, and the gateway it would be good on: what
/// [`GrantPolicyEvaluator::may_issue`] asks about.
///
/// [`GrantPolicyEvaluator::may_issue`]: crate::application::ports::GrantPolicyEvaluator::may_issue
pub(super) fn recipient(
    schema: &Schema,
    kind: PrincipalKind,
    role: MembershipRole,
    gateway: &Resource,
) -> Result<(EntityUid, EntityUid, Entities), AccessError> {
    let (kind, role) = (attributes::kind(kind), attributes::role(role));
    principal_and_resource(
        schema,
        cedar_uid("Nessa::Recipient", &format!("{kind}:{role}"))?,
        [("kind", kind), ("role", role)],
        gateway,
    )
}

/// A principal with string attributes and the resource it is asked about, as
/// schema-validated entities.
fn principal_and_resource<const N: usize>(
    schema: &Schema,
    principal_uid: EntityUid,
    attributes: [(&str, &str); N],
    resource: &Resource,
) -> Result<(EntityUid, EntityUid, Entities), AccessError> {
    let principal = Entity::new(
        principal_uid.clone(),
        attributes
            .into_iter()
            .map(|(key, value)| {
                (
                    key.to_owned(),
                    RestrictedExpression::new_string(value.to_owned()),
                )
            })
            .collect::<HashMap<_, _>>(),
        Default::default(),
    )
    .map_err(|_| AccessError::Unavailable)?;
    let resource_uid = cedar_uid("Nessa::Resource", resource.id().as_str())?;
    let resource = Entity::new(
        resource_uid.clone(),
        ResourceAttributes::from(resource).into_cedar(),
        Default::default(),
    )
    .map_err(|_| AccessError::Unavailable)?;
    let entities = Entities::from_entities([principal, resource], Some(schema))
        .map_err(|_| AccessError::Unavailable)?;
    Ok((principal_uid, resource_uid, entities))
}
