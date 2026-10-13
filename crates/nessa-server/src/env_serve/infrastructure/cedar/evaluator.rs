//! The lease policy bundle, loaded and validated once, and each lease
//! decided by it.
use crate::env_serve::{
    application::{LeaseAsk, LeaseDecision, LeaseFacts, LeasePolicyEvaluator},
    domain::{Agents, Peer, PeerGrant},
};
use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema, ValidationMode, Validator,
};
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

const SCHEMA: &str = include_str!("schema.json");
const POLICIES: &str = include_str!("policies.cedar");

/// The checked-in lease policy, strictly validated.
pub(crate) struct CedarLeasePolicy {
    schema: Schema,
    policies: PolicySet,
    authorizer: Authorizer,
}

/// A step of building a request or its entities that failed: the lease is
/// denied.
struct Unavailable;

impl CedarLeasePolicy {
    /// Load and strictly validate the checked-in bundle.
    ///
    /// # Errors
    /// The bundle does not parse, does not validate without warnings, or
    /// holds a policy without an `@id`; the message says which.
    pub(crate) fn new() -> Result<Self, String> {
        Self::from_sources(SCHEMA, POLICIES)
    }

    fn from_sources(schema: &str, policies: &str) -> Result<Self, String> {
        let schema = Schema::from_json_str(schema)
            .map_err(|error| format!("the lease policy schema does not parse: {error}"))?;
        let policies = PolicySet::from_str(policies)
            .map_err(|error| format!("the lease policies do not parse: {error}"))?;
        if let Some(unnamed) = policies
            .policies()
            .find(|policy| policy.annotation("id").is_none_or(str::is_empty))
        {
            return Err(format!("a lease policy has no @id: {}", unnamed.id()));
        }
        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed_without_warnings() {
            let found: Vec<String> = validation
                .validation_errors()
                .map(ToString::to_string)
                .chain(validation.validation_warnings().map(ToString::to_string))
                .collect();
            return Err(format!(
                "the lease policies do not validate: {}",
                found.join("; ")
            ));
        }
        Ok(Self {
            schema,
            policies,
            authorizer: Authorizer::new(),
        })
    }

    fn decide(&self, facts: &LeaseFacts<'_>) -> Result<LeaseDecision, Unavailable> {
        let (action, resource) = match facts.ask {
            LeaseAsk::Agent(name) => (
                "agent.lease",
                Entity::new(
                    uid("NessaEnv::Agent", name)?,
                    HashMap::from([(
                        "name".to_owned(),
                        RestrictedExpression::new_string(name.to_owned()),
                    )]),
                    HashSet::new(),
                )
                .map_err(|_| Unavailable)?,
            ),
            LeaseAsk::Command => (
                "command.run",
                Entity::new_no_attrs(uid("NessaEnv::Command", "command")?, HashSet::new()),
            ),
        };
        let principal = peer(facts.peer, facts.entry)?;
        let action = uid("NessaEnv::Action", action)?;
        let request = Request::new(
            principal.uid(),
            action.clone(),
            resource.uid(),
            Context::empty(),
            Some(&self.schema),
        )
        .map_err(|_| Unavailable)?;
        let entities = Entities::from_entities([principal, resource], Some(&self.schema))
            .map_err(|_| Unavailable)?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if let Some(error) = response.diagnostics().errors().next() {
            tracing::error!(%error, "the lease policy could not be evaluated; the lease is refused");
            return Err(Unavailable);
        }
        let policies = response
            .diagnostics()
            .reason()
            .filter_map(|id| self.policies.annotation(id, "id"))
            .map(str::to_owned)
            .collect();
        Ok(match response.decision() {
            Decision::Allow => LeaseDecision::Allow { policies },
            Decision::Deny => LeaseDecision::Deny { policies },
        })
    }
}

impl LeasePolicyEvaluator for CedarLeasePolicy {
    fn evaluate(&self, facts: &LeaseFacts<'_>) -> LeaseDecision {
        self.decide(facts)
            .unwrap_or(LeaseDecision::Deny { policies: vec![] })
    }
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, Unavailable> {
    let entity_type = EntityTypeName::from_str(entity_type).map_err(|_| Unavailable)?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

/// The peer as the schema's `Peer`: its kind, and its entry as written.
fn peer(peer: &Peer, entry: Option<&PeerGrant>) -> Result<Entity, Unavailable> {
    let (kind, id) = match peer {
        Peer::Ssh => ("ssh", "ssh".to_owned()),
        Peer::Gateway { pin } => (
            "gateway",
            pin.iter().fold("gateway:".to_owned(), |mut id, byte| {
                id.push_str(&format!("{byte:02x}"));
                id
            }),
        ),
    };
    let (all_agents, agents, commands) = match entry {
        None => (false, Vec::new(), false),
        Some(entry) => {
            let (all, named) = match entry.agents() {
                Agents::All => (true, Vec::new()),
                Agents::Named(named) => (
                    false,
                    named
                        .iter()
                        .map(|name| RestrictedExpression::new_string(name.as_str().to_owned()))
                        .collect(),
                ),
            };
            (all, named, entry.commands().is_some())
        }
    };
    Entity::new(
        uid("NessaEnv::Peer", &id)?,
        HashMap::from([
            (
                "kind".to_owned(),
                RestrictedExpression::new_string(kind.to_owned()),
            ),
            (
                "hasEntry".to_owned(),
                RestrictedExpression::new_bool(entry.is_some()),
            ),
            (
                "allAgents".to_owned(),
                RestrictedExpression::new_bool(all_agents),
            ),
            ("agents".to_owned(), RestrictedExpression::new_set(agents)),
            (
                "commands".to_owned(),
                RestrictedExpression::new_bool(commands),
            ),
        ]),
        HashSet::new(),
    )
    .map_err(|_| Unavailable)
}

#[cfg(test)]
#[path = "../../../../tests/env_serve/lease_policy.rs"]
mod tests;
