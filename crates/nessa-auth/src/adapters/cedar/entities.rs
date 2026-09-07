//! Translate typed Nessa values to the checked-in Cedar schema.
//!
//! Attribute keys are Cedar record field names. Organization IDs and roles become
//! schema String values here; membership activity remains a Boolean. These wire
//! representations never replace the domain identifiers or MembershipRole enum.

use crate::{
    application::ports::{AccessError, AccessSnapshot},
    domain::{AuthContext, Resource},
};
use cedar_policy::{Entities, Entity, EntityId, EntityTypeName, EntityUid, Schema};
use std::str::FromStr;

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
        ActorAttributes::from(&snapshot.membership).into_cedar(),
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
