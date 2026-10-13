//! Translate typed Nessa values to the checked-in Cedar schema.
//!
//! Attribute keys are Cedar record field names. Organization IDs and roles become
//! schema String values here; membership activity remains a Boolean. These wire
//! representations never replace the domain identifiers or MembershipRole enum.

use crate::{
    application::ports::{AccessError, AccessSnapshot},
    domain::{AuthContext, PrincipalKind, Resource},
};
use cedar_policy::{
    Entities, Entity, EntityId, EntityTypeName, EntityUid, RestrictedExpression, Schema,
};
use std::{collections::HashMap, str::FromStr};

use super::attributes::{ActorAttributes, ResourceAttributes};

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
/// what [`GrantPolicyEvaluator`] asks about.
///
/// [`GrantPolicyEvaluator`]: crate::application::ports::GrantPolicyEvaluator
pub(super) fn holding(
    schema: &Schema,
    kind: PrincipalKind,
    resource: &Resource,
) -> Result<(EntityUid, EntityUid, Entities), AccessError> {
    let kind = match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent => "agent",
        PrincipalKind::Integration => "integration",
        PrincipalKind::Gateway => "gateway",
    };
    let holder_uid = cedar_uid("Nessa::Holder", kind)?;
    let holder = Entity::new(
        holder_uid.clone(),
        HashMap::from([(
            "kind".to_owned(),
            RestrictedExpression::new_string(kind.to_owned()),
        )]),
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
    let entities = Entities::from_entities([holder, resource], Some(schema))
        .map_err(|_| AccessError::Unavailable)?;
    Ok((holder_uid, resource_uid, entities))
}
