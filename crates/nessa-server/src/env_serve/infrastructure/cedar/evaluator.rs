//! The lease policy bundle, loaded and validated once, and each lease
//! decided by it.
use crate::env_serve::{
    application::{Denial, LeaseAsk, LeaseDecision, LeaseFacts, LeasePolicyEvaluator},
    domain::{Agents, Peer},
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

/// A step of building a request or its entities, or of evaluating it, that
/// failed, and how: the lease is denied, and this logged.
struct Unavailable(String);

impl Unavailable {
    /// `step` failed with `error`.
    fn at(step: &str, error: impl std::fmt::Display) -> Self {
        Self(format!("{step}: {error}"))
    }
}

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
                .map_err(|error| Unavailable::at("the agent entity", error))?,
            ),
            LeaseAsk::Command => (
                "command.run",
                Entity::new_no_attrs(uid("NessaEnv::Command", "command")?, HashSet::new()),
            ),
            LeaseAsk::Update => (
                "build.update",
                Entity::new_no_attrs(uid("NessaEnv::Build", "build")?, HashSet::new()),
            ),
        };
        let principal = peer(facts)?;
        let action = uid("NessaEnv::Action", action)?;
        let request = Request::new(
            principal.uid(),
            action.clone(),
            resource.uid(),
            Context::empty(),
            Some(&self.schema),
        )
        .map_err(|error| Unavailable::at("the request", error))?;
        let entities = Entities::from_entities([principal, resource], Some(&self.schema))
            .map_err(|error| Unavailable::at("the entities", error))?;
        let response = self
            .authorizer
            .is_authorized(&request, &self.policies, &entities);
        if let Some(error) = response.diagnostics().errors().next() {
            return Err(Unavailable::at("evaluation", error));
        }
        let policies: Vec<String> = response
            .diagnostics()
            .reason()
            .filter_map(|id| self.policies.annotation(id, "id"))
            .map(str::to_owned)
            .collect();
        Ok(match response.decision() {
            Decision::Allow => LeaseDecision::Allow { policies },
            // Cedar denies by a forbid it names, or by no permit at all.
            Decision::Deny if policies.is_empty() => LeaseDecision::Deny(Denial::NotPermitted),
            Decision::Deny => LeaseDecision::Deny(Denial::Forbidden { policies }),
        })
    }
}

impl LeasePolicyEvaluator for CedarLeasePolicy {
    fn evaluate(&self, facts: &LeaseFacts<'_>) -> LeaseDecision {
        self.decide(facts).unwrap_or_else(|Unavailable(error)| {
            tracing::error!(%error, ask = ?facts.ask, "the lease policy could not decide; the lease is refused");
            LeaseDecision::Deny(Denial::Unavailable)
        })
    }
}

fn uid(entity_type: &str, id: &str) -> Result<EntityUid, Unavailable> {
    let entity_type = EntityTypeName::from_str(entity_type)
        .map_err(|error| Unavailable::at("an entity type", error))?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

/// The peer as the schema's `Peer`: its kind, whether it has an entry, and
/// the grant it holds.
fn peer(facts: &LeaseFacts<'_>) -> Result<Entity, Unavailable> {
    let (kind, id) = match facts.peer {
        Peer::Ssh => ("ssh", "ssh".to_owned()),
        Peer::Gateway { pin } => (
            "gateway",
            pin.iter().fold("gateway:".to_owned(), |mut id, byte| {
                id.push_str(&format!("{byte:02x}"));
                id
            }),
        ),
    };
    let grant = facts.grant;
    let (all_agents, agents) = match grant.agents() {
        Agents::All => (true, Vec::new()),
        Agents::Named(named) => (
            false,
            named
                .iter()
                .map(|name| RestrictedExpression::new_string(name.as_str().to_owned()))
                .collect(),
        ),
    };
    let commands = grant.commands().is_some();
    Entity::new(
        uid("NessaEnv::Peer", &id)?,
        HashMap::from([
            (
                "kind".to_owned(),
                RestrictedExpression::new_string(kind.to_owned()),
            ),
            (
                "hasEntry".to_owned(),
                RestrictedExpression::new_bool(facts.written),
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
    .map_err(|error| Unavailable::at("the peer entity", error))
}

#[cfg(test)]
#[path = "../../../../tests/env_serve/lease_policy.rs"]
mod tests;
