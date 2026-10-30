//! What every evaluator in this bundle shares: entity ids, the two
//! principals a turn runs for, and asking Cedar for a decision that names
//! its policies.

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;

use crate::conversation::application::{ToolDecision, ToolPolicyUnavailable};
use crate::conversation::domain::{ToolChoice, ToolPolicy, ToolPreset};

use super::bundle::Bundle;

pub(super) const OWNER: &str = "Nessa::Owner";
pub(super) const AGENT: &str = "Nessa::Agent";
pub(super) const GRANT: &str = "Nessa::Grant";
pub(super) const ACTION: &str = "Nessa::Action";
/// A grant whose policy is its own rules names this as its preset, so no
/// checked-in preset policy applies to it.
const RULES: &str = "rules";

/// One question for Cedar: who asks, what, of what, with every other entity
/// the policies read and the request's context.
pub(super) struct Ask {
    pub(super) principal: Entity,
    pub(super) action: &'static str,
    pub(super) resource: Entity,
    pub(super) others: Vec<Entity>,
    pub(super) context: Context,
}

/// Cedar's answer to `ask` under `policies`, with the ids of the policies
/// that decided it, sorted. Any error in the request, the entities or any
/// policy's evaluation is `ToolPolicyUnavailable`.
pub(super) fn decide(
    authorizer: &Authorizer,
    bundle: &Bundle,
    policies: &PolicySet,
    ask: Ask,
) -> Result<ToolDecision, ToolPolicyUnavailable> {
    let schema = bundle.schema();
    let request = Request::new(
        ask.principal.uid(),
        uid(ACTION, ask.action)?,
        ask.resource.uid(),
        ask.context,
        Some(schema),
    )
    .map_err(|_| ToolPolicyUnavailable)?;
    let mut entities = vec![ask.principal, ask.resource];
    entities.extend(ask.others);
    let entities =
        Entities::from_entities(entities, Some(schema)).map_err(|_| ToolPolicyUnavailable)?;
    let response = authorizer.is_authorized(&request, policies, &entities);
    // Cedar skips a policy whose condition errors, a forbid included, so
    // any error refuses the whole decision rather than let it open.
    if response.diagnostics().errors().next().is_some() {
        return Err(ToolPolicyUnavailable);
    }
    let mut rules: Vec<String> = response
        .diagnostics()
        .reason()
        .map(ToString::to_string)
        .collect();
    rules.sort();
    Ok(match response.decision() {
        Decision::Allow => ToolDecision::Allow { rules },
        Decision::Deny => ToolDecision::Refuse { rules },
    })
}

pub(super) fn uid(entity_type: &str, id: &str) -> Result<EntityUid, ToolPolicyUnavailable> {
    let entity_type = EntityTypeName::from_str(entity_type).map_err(|_| ToolPolicyUnavailable)?;
    Ok(EntityUid::from_type_name_and_id(
        entity_type,
        EntityId::new(id),
    ))
}

/// An entity of `entity_type` with `attributes`.
pub(super) fn entity(
    entity_type: &str,
    id: &str,
    attributes: impl IntoIterator<Item = (&'static str, RestrictedExpression)>,
) -> Result<Entity, ToolPolicyUnavailable> {
    Entity::new(
        uid(entity_type, id)?,
        attributes
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect::<HashMap<_, _>>(),
        HashSet::new(),
    )
    .map_err(|_| ToolPolicyUnavailable)
}

/// A set of strings, as an attribute's value.
pub(super) fn strings<'a>(values: impl IntoIterator<Item = &'a String>) -> RestrictedExpression {
    RestrictedExpression::new_set(
        values
            .into_iter()
            .map(|value| RestrictedExpression::new_string(value.clone())),
    )
}

/// The conversation's owner.
pub(super) fn owner() -> Result<Entity, ToolPolicyUnavailable> {
    entity(OWNER, "owner", [])
}

/// An agent-authored turn; its source identity is retained in the initiation.
pub(super) fn agent() -> Result<Entity, ToolPolicyUnavailable> {
    entity(AGENT, "agent", [])
}

/// A Drive grant, with every fact of it the caller asks about: its tool
/// policy's preset (`rules` for its own rules), and, for the command policy
/// alone, the hosts its policy names for its commands (an empty set when it
/// names none). One builder for every evaluator, so the entity is the same
/// wherever it is read.
pub(super) fn grant(
    grant: &str,
    policy: &ToolPolicy,
    command_hosts: Option<&BTreeSet<String>>,
) -> Result<Entity, ToolPolicyUnavailable> {
    let preset = match policy.tools() {
        ToolChoice::Preset(preset) => preset_name(*preset),
        ToolChoice::Rules(_) => RULES,
    };
    let mut attributes = vec![(
        "toolPreset",
        RestrictedExpression::new_string(preset.to_owned()),
    )];
    if let Some(hosts) = command_hosts {
        attributes.push(("commandHosts", strings(hosts)));
    }
    entity(GRANT, grant, attributes)
}

/// A preset's name, as `tools.cedar` compares it.
fn preset_name(preset: ToolPreset) -> &'static str {
    match preset {
        ToolPreset::ReadOnlyTools => "read_only_tools",
        ToolPreset::NoShell => "no_shell",
        ToolPreset::AskEverything => "ask_everything",
        ToolPreset::All => "all",
    }
}
